import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "./test/svelte";
import type { Share as ShareRow, Sharing } from "./hosted/api";
import Share from "./ui/Share.svelte";
import ShareDialog from "./ui/ShareDialog.svelte";
import { shareOpen } from "./ui/more-menu.svelte";
import { linkRequest, parseLogin, shareDetail, shareName, subject, suggest } from "./ui/share-model";

const ID = "7q3k9mzx2b4t";
const BASE = `/api/hosted/artifacts/${ID}`;
type Open = Extract<Sharing, { can_share: true }>;
const sam: ShareRow = { id: "s1", access: "viewer", github_login: "sam-lee", person: "u_sam", name: "Sam Lee", pending: false };
const octo: ShareRow = { id: "s2", access: "commenter", github_login: "octo", person: null, name: null, pending: true };
const open = (over: Partial<Open> = {}): Open => ({
  artifact_id: ID, can_share: true, general: "restricted", link_access: null, url: `https://clax.test/a/${ID}`,
  people: [sam], suggestions: [{ person: "u_sam", name: "Sam Lee", github_login: "sam-lee" }, { person: "u_mia", name: "Mia Chen", github_login: null }],
  ...over,
});

/** `fetch` answering the sharing routes from `state`, recording each change. */
function serve(state: { sharing: Sharing }, sent: { method: string; url: string; body: unknown }[] = []) {
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    if (url === `${BASE}/sharing` && method === "GET") return new Response(JSON.stringify(state.sharing));
    if (url.startsWith(BASE)) {
      sent.push({ method, url, body: init?.body ? JSON.parse(String(init.body)) : null });
      return new Response(JSON.stringify({ share: sam }));
    }
    return new Response("{}", { status: 404 });
  }));
  return sent;
}

const until = async <T,>(f: () => T | null | undefined | false): Promise<T> => {
  for (let i = 0; i < 200; i++) {
    flush();
    const v = f();
    if (v) return v;
    await new Promise(r => setTimeout(r, 5));
  }
  throw new Error("not reached");
};

afterEach(() => { vi.unstubAllGlobals(); document.body.innerHTML = ""; });

describe("share model", () => {
  it("reads a GitHub login as the daemon does", () => {
    expect(parseLogin("@Octo-Cat ")).toBe("octo-cat");
    expect(parseLogin("x".repeat(39))).toBe("x".repeat(39));
    for (const bad of ["", "@", "-lead", "a b", "a_b", "x".repeat(40), "é"]) expect(parseLogin(bad), bad).toBeNull();
  });

  it("names a share by its person, else its login, and says when it waits", () => {
    expect([shareName(sam), shareDetail(sam)]).toEqual(["Sam Lee", "@sam-lee"]);
    expect(shareName(octo)).toBe("@octo");
    expect(shareDetail(octo)).toMatch(/once they sign in with GitHub/);
  });

  it("suggests people not shared with yet, by a word of their name or login", () => {
    const people = open().suggestions;
    expect(suggest("mi", people, [sam]).map(c => c.person)).toEqual(["u_mia"]);
    expect(suggest("chen", people, []).map(c => c.person)).toEqual(["u_mia"]);
    expect(suggest("@lee", people, []).map(c => c.person)).toEqual(["u_sam"]);
    expect(suggest("lee", people, [sam]), "Sam already has access").toEqual([]);
    expect(suggest("  ", people, [])).toEqual([]);
  });

  it("names a chosen person, else the login typed", () => {
    expect(subject("@Octo", null)).toEqual({ github_login: "octo" });
    expect(subject("Mia Chen", { person: "u_mia", name: "Mia Chen", github_login: null })).toEqual({ person: "u_mia" });
    expect(subject("Mia Chen", null)).toBeNull();
  });

  it("maps general access onto the link request", () => {
    expect(linkRequest("restricted", "commenter")).toEqual({ access: null, listed: false });
    expect(linkRequest("link", "commenter")).toEqual({ access: "commenter", listed: false });
    expect(linkRequest("public", "viewer")).toEqual({ access: "viewer", listed: true });
  });
});

describe("Share", () => {
  it("shows nothing to who may not share, or on a daemon without sharing", async () => {
    serve({ sharing: { artifact_id: ID, can_share: false } });
    const m = mount(Share, { id: ID, title: "Plan" });
    await new Promise(r => setTimeout(r, 20));
    flush();
    expect(m.root.querySelector("button.share")).toBeNull();
    expect(shareOpen.current).toBeNull();
    m.unmount();
    vi.stubGlobal("fetch", vi.fn(async () => new Response("{}", { status: 404 })));
    const l = mount(Share, { id: ID, title: "Plan" });
    await new Promise(r => setTimeout(r, 20));
    flush();
    expect(l.root.querySelector("button.share")).toBeNull();
    l.unmount();
  });

  it("shows Share to a sharer, offers the more menu's item, and opens the dialog", async () => {
    serve({ sharing: open() });
    const m = mount(Share, { id: ID, title: "Plan" });
    const b = await until(() => m.root.querySelector<HTMLButtonElement>("button.share"));
    expect(b.classList.contains("hide-sm")).toBe(true);
    expect(shareOpen.current).not.toBeNull();
    flush(() => b.click());
    const dialog = await until(() => document.querySelector("[role=dialog]"));
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(dialog.querySelector("h2")?.textContent).toBe("Share Plan");
    m.unmount();
    expect(shareOpen.current).toBeNull();
  });
});

describe("Share dialog", () => {
  it("adds a login, changes and removes a person, and sets general access", async () => {
    const state = { sharing: open() as Sharing };
    const sent = serve(state);
    const writes: string[] = [];
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText: async (t: string) => { writes.push(t); } } });
    const onClose = vi.fn();
    const m = mount(ShareDialog, { id: ID, title: "Plan", initial: open(), onClose });
    const root = m.root;
    const idle = () => until(() => root.querySelector("[role=dialog]")!.getAttribute("aria-busy") === "false");
    expect(document.activeElement?.getAttribute("name"), "focus starts in the add field").toBe("who");
    // The people with access, each with their access.
    const rows = root.querySelectorAll(".shared li");
    expect(rows.length).toBe(1);
    expect(rows[0].textContent).toContain("Sam Lee");
    expect(root.querySelector<HTMLSelectElement>("select[aria-label='Access for Sam Lee']")!.value).toBe("viewer");
    expect(root.querySelector("[data-testid=general-note]")!.textContent).toBe("Only the people added can open it.");
    // A bad login is refused here, with no request.
    const who = root.querySelector<HTMLInputElement>("input[name=who]")!;
    const form = root.querySelector<HTMLFormElement>("form.add")!;
    flush(() => { who.value = "not a login"; who.dispatchEvent(new Event("input", { bubbles: true })); });
    flush(() => form.requestSubmit());
    expect(root.querySelector("[role=alert]")!.textContent).toMatch(/GitHub login/);
    expect(sent).toEqual([]);
    // Add @octo, Can comment.
    flush(() => { who.value = "@Octo"; who.dispatchEvent(new Event("input", { bubbles: true })); });
    const access = root.querySelector<HTMLSelectElement>("select[aria-label='Access for the person added']")!;
    flush(() => { access.value = "commenter"; access.dispatchEvent(new Event("change", { bubbles: true })); });
    state.sharing = open({ people: [sam, octo] });
    flush(() => form.requestSubmit());
    await until(() => root.querySelectorAll(".shared li").length === 2);
    await idle();
    expect(sent.at(-1)).toEqual({ method: "PUT", url: `${BASE}/shares`, body: { github_login: "octo", access: "commenter" } });
    expect(root.querySelectorAll(".shared li")[1].textContent).toMatch(/@octo.*once they sign in with GitHub/);
    expect(who.value, "the field clears").toBe("");
    // A suggestion is picked by name and shared by person.
    flush(() => { who.value = "mi"; who.dispatchEvent(new Event("input", { bubbles: true })); });
    const hint = await until(() => root.querySelector<HTMLButtonElement>(".hints button"));
    expect(hint.textContent).toBe("Mia Chen");
    flush(() => hint.click());
    flush(() => form.requestSubmit());
    await until(() => sent.length === 2);
    await idle();
    expect(sent[1].body).toEqual({ person: "u_mia", access: "commenter" });
    // Sam becomes an editor; then is removed.
    const samAccess = root.querySelector<HTMLSelectElement>("select[aria-label='Access for Sam Lee']")!;
    flush(() => { samAccess.value = "editor"; samAccess.dispatchEvent(new Event("change", { bubbles: true })); });
    await until(() => sent.length === 3);
    await idle();
    expect(sent[2].body).toEqual({ person: "u_sam", access: "editor" });
    state.sharing = open({ people: [octo] });
    flush(() => root.querySelector<HTMLButtonElement>("button[aria-label='Remove Sam Lee']")!.click());
    await until(() => sent.length === 4 && root.querySelectorAll(".shared li").length === 1);
    await idle();
    expect(sent[3]).toEqual({ method: "DELETE", url: `${BASE}/shares/s1`, body: {} });
    // Public, Can view.
    state.sharing = open({ people: [octo], general: "public", link_access: "viewer" });
    const general = root.querySelector<HTMLSelectElement>("select[aria-label='General access']")!;
    flush(() => { general.value = "public"; general.dispatchEvent(new Event("change", { bubbles: true })); });
    await until(() => sent.length === 5);
    await until(() => root.querySelector("select[aria-label='What anyone with the link may do']"));
    expect(sent[4]).toEqual({ method: "PUT", url: `${BASE}/link`, body: { access: "viewer", listed: true } });
    expect(root.querySelector("[data-testid=general-note]")!.textContent).toMatch(/listed on this Clax's public page/);
    // Copy link, then Escape closes.
    const copy = root.querySelector<HTMLButtonElement>("button.copy")!;
    flush(() => copy.click());
    await until(() => copy.textContent === "Copied");
    expect(writes).toEqual([`https://clax.test/a/${ID}`]);
    root.querySelector<HTMLElement>("[role=dialog]")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(onClose).toHaveBeenCalledTimes(1);
    m.unmount();
  });

  it("says what the daemon refused", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ error: { code: "forbidden", message: "only the owner or the page's maker shares it" } }), { status: 403 })));
    const m = mount(ShareDialog, { id: ID, title: "Plan", initial: open(), onClose: vi.fn() });
    const who = m.root.querySelector<HTMLInputElement>("input[name=who]")!;
    flush(() => { who.value = "octo"; who.dispatchEvent(new Event("input", { bubbles: true })); });
    flush(() => m.root.querySelector<HTMLFormElement>("form.add")!.requestSubmit());
    const err = await until(() => m.root.querySelector("[role=alert]"));
    expect(err.textContent).toBe("only the owner or the page's maker shares it");
    m.unmount();
  });
});
