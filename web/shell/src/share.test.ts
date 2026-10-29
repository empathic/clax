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
  public_gallery: true, comments: true, people: [sam], suggestions: [{ person: "u_sam", name: "Sam Lee", github_login: "sam-lee" }, { person: "u_mia", name: "Mia Chen", github_login: null }],
  ...over,
});

/** `fetch` answering the sharing routes from `state`, recording each change. */
function serve(state: { sharing: Sharing }, sent: { method: string; url: string; body: unknown }[] = []) {
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    if (url === `${BASE}/sharing` && method === "GET") return new Response(JSON.stringify(state.sharing));
    if (url.startsWith(BASE) || url === `/api/artifacts/${ID}`) {
      const body = init?.body ? JSON.parse(String(init.body)) as Record<string, unknown> : null;
      sent.push({ method, url, body });
      if (url === `${BASE}/comments` && method === "PUT") return new Response(JSON.stringify({ artifact_id: ID, comments: body?.on }));
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

  it("names the person a typed name is, never a GitHub login of that word", () => {
    const people = [{ person: "u_sam", name: "Sam", github_login: null }, { person: "u_jo", name: "Jo", github_login: null }, { person: "u_jo2", name: "jo", github_login: null }];
    expect(subject("Sam", null, people)).toEqual({ person: "u_sam" });
    expect(subject(" sam ", null, people)).toEqual({ person: "u_sam" });
    expect(subject("@sam", null, people)).toEqual({ github_login: "sam" });
    expect(subject("Jo", null, people)).toBeNull();
    expect(subject("octo", null, people)).toEqual({ github_login: "octo" });
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
  it("keeps what is typed while an add is in flight, clearing only the text it sent", async () => {
    let release = () => {};
    const gate = new Promise<void>(r => { release = r; });
    vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
      const method = init?.method ?? "GET";
      if (url === `${BASE}/sharing` && method === "GET") return new Response(JSON.stringify(open({ people: [sam, octo] })));
      if (url === `${BASE}/shares`) { await gate; return new Response(JSON.stringify({ share: octo })); }
      return new Response("{}", { status: 404 });
    }));
    const m = mount(ShareDialog, { id: ID, title: "Plan", initial: open(), onClose: vi.fn() });
    const root = m.root;
    const who = root.querySelector<HTMLInputElement>("input[name=who]")!;
    const type = (v: string) => flush(() => { who.value = v; who.dispatchEvent(new Event("input", { bubbles: true })); });
    type("@octo");
    flush(() => root.querySelector<HTMLFormElement>("form.add")!.requestSubmit());
    await until(() => root.querySelector("[role=dialog]")!.getAttribute("aria-busy") === "true");
    type("@next");
    release();
    await until(() => root.querySelectorAll(".shared li").length === 2);
    await until(() => root.querySelector("[role=dialog]")!.getAttribute("aria-busy") === "false");
    flush();
    expect(who.value, "the next name typed stays").toBe("@next");
    m.unmount();
  });

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

  it("share dialog toggles comments", async () => {
    const state = { sharing: open() as Sharing };
    const sent = serve(state);
    const onComments = vi.fn();
    const m = mount(ShareDialog, { id: ID, title: "Plan", initial: open(), onClose: vi.fn(), onComments });
    const root = m.root;
    const sw = root.querySelector<HTMLInputElement>("input[role=switch]")!;
    expect(sw.labels?.[0]?.textContent?.trim()).toBe("Comments");
    expect(sw.checked).toBe(true);
    expect(root.querySelector("[data-testid=comments-note]")!.textContent).toBe("People who can comment may comment.");
    // The note describes the switch to assistive tech.
    expect(document.getElementById(sw.getAttribute("aria-describedby")!)).toBe(root.querySelector("[data-testid=comments-note]"));
    // Above the people with access.
    const people = Array.from(root.querySelectorAll("h3")).find(h => h.textContent === "People with access")!;
    expect(sw.compareDocumentPosition(people) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    state.sharing = open({ comments: false });
    flush(() => sw.click());
    await until(() => sent.length === 1);
    expect(sent[0]).toEqual({ method: "PUT", url: `${BASE}/comments`, body: { on: false } });
    await until(() => root.querySelector("[data-testid=comments-note]")!.textContent === "Off: nobody can comment, whatever their access.");
    expect(sw.checked).toBe(false);
    expect(onComments).toHaveBeenLastCalledWith(false);
    state.sharing = open({ comments: true });
    flush(() => sw.click());
    await until(() => sent.length === 2);
    expect(sent[1].body).toEqual({ on: true });
    await until(() => root.querySelector("[data-testid=comments-note]")!.textContent === "People who can comment may comment.");
    expect(sw.checked).toBe(true);
    expect(onComments).toHaveBeenLastCalledWith(true);
    m.unmount();
  });

  it("share dialog shows and tells a Comments change that took though its reload failed, and takes no tap while busy", async () => {
    const sent: { method: string; url: string; body: unknown }[] = [];
    let reload: ((r: Response) => void) | null = null;
    vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
      const method = init?.method ?? "GET";
      if (url === `${BASE}/comments` && method === "PUT") {
        const body = JSON.parse(String(init!.body)) as { on: boolean };
        sent.push({ method, url, body });
        return new Response(JSON.stringify({ artifact_id: ID, comments: body.on }));
      }
      // The dialog's reload hangs until the test answers it.
      if (url === `${BASE}/sharing`) return new Promise<Response>(r => { reload = r; });
      return new Response("{}", { status: 404 });
    }));
    const onComments = vi.fn();
    const m = mount(ShareDialog, { id: ID, title: "Plan", initial: open(), onClose: vi.fn(), onComments });
    const root = m.root;
    const sw = root.querySelector<HTMLInputElement>("input[role=switch]")!;
    flush(() => sw.click());
    await until(() => sent.length === 1);
    // While the change is in flight the switch takes no tap.
    await until(() => reload);
    expect(sw.disabled).toBe(true);
    reload!(new Response(JSON.stringify({ error: { code: "internal", message: "db locked" } }), { status: 500 }));
    await until(() => !sw.disabled);
    expect(sw.checked).toBe(false);
    expect(root.querySelector("[data-testid=comments-note]")!.textContent).toBe("Off: nobody can comment, whatever their access.");
    expect(onComments).toHaveBeenLastCalledWith(false);
    m.unmount();
  });

  it("takes no General access change while a Comments change is in flight, and never shows one it did not send", async () => {
    const sent: { method: string; url: string; body: unknown }[] = [];
    let reload: ((r: Response) => void) | null = null;
    vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
      const method = init?.method ?? "GET";
      const body = init?.body ? JSON.parse(String(init.body)) as Record<string, unknown> : null;
      if (method !== "GET") sent.push({ method, url, body });
      if (url === `${BASE}/comments`) return new Response(JSON.stringify({ artifact_id: ID, comments: body?.on }));
      if (url === `${BASE}/link`) return new Response(JSON.stringify({ artifact_id: ID, access: body?.access, listed: body?.listed, url: "" }));
      // The reload after the Comments change hangs until the test answers it; later ones answer at once.
      if (url === `${BASE}/sharing`) {
        if (sent.length === 1) return new Promise<Response>(r => { reload = r; });
        return new Response(JSON.stringify(open({ comments: false, general: "link", link_access: "viewer" })));
      }
      return new Response("{}", { status: 404 });
    }));
    const m = mount(ShareDialog, { id: ID, title: "Plan", initial: open(), onClose: vi.fn() });
    const root = m.root;
    const general = root.querySelector<HTMLSelectElement>("select[aria-label='General access']")!;
    expect(general.disabled).toBe(false);
    flush(() => root.querySelector<HTMLInputElement>("input[role=switch]")!.click());
    await until(() => reload);
    // The Comments change is held and its note shows; its reload is still on its way.
    expect(root.querySelector("[data-testid=comments-note]")!.textContent).toBe("Off: nobody can comment, whatever their access.");
    expect(general.disabled).toBe(true);
    // A change that reaches the select anyway is not sent, and the select shows what the daemon holds.
    flush(() => { general.value = "link"; general.dispatchEvent(new Event("change", { bubbles: true })); });
    await until(() => general.value === "restricted");
    expect(sent.map(s => s.url)).toEqual([`${BASE}/comments`]);
    expect(root.querySelector("select[aria-label='What anyone with the link may do']")).toBeNull();
    reload!(new Response(JSON.stringify(open({ comments: false }))));
    await until(() => !general.disabled);
    // Once the reload lands, the change takes.
    flush(() => { general.value = "link"; general.dispatchEvent(new Event("change", { bubbles: true })); });
    const linkSel = await until(() => root.querySelector<HTMLSelectElement>("select[aria-label='What anyone with the link may do']"));
    expect(sent[1]).toEqual({ method: "PUT", url: `${BASE}/link`, body: { access: "viewer", listed: false } });
    expect(linkSel.disabled).toBe(false);
    m.unmount();
  });

  it("offers no Public while the public gallery is off", () => {
    const state = { sharing: open({ public_gallery: false }) as Sharing };
    serve(state);
    const m = mount(ShareDialog, { id: ID, title: "Plan", initial: open({ public_gallery: false }), onClose: vi.fn() });
    const general = m.root.querySelector<HTMLSelectElement>("select[aria-label='General access']")!;
    expect([...general.options].map(o => o.textContent)).toEqual(["Restricted", "Anyone with the link"]);
    m.unmount();
  });

  it("opens the site to the world and closes it, showing its address", async () => {
    const site_url = `https://${ID}.clax.run/`;
    const state = { sharing: open({ site_access: "private", site_url }) as Sharing };
    const sent = serve(state);
    const m = mount(ShareDialog, { id: ID, title: "Plan", initial: open({ site_access: "private", site_url }), onClose: vi.fn() });
    const root = m.root;
    const box = root.querySelector<HTMLInputElement>("input[name=world]")!;
    const note = () => root.querySelector("[data-testid=site-note]")!.textContent ?? "";
    expect(box.checked).toBe(false);
    expect(note()).toMatch(/^Only the people with access/);
    expect(root.querySelector<HTMLAnchorElement>("a.site")!.getAttribute("href")).toBe(site_url);
    state.sharing = open({ site_access: "world", site_url });
    flush(() => box.click());
    await until(() => note().startsWith("Anyone"));
    expect(sent).toEqual([{ method: "PATCH", url: `/api/artifacts/${ID}`, body: { site_access: "world" } }]);
    expect(box.checked).toBe(true);
    state.sharing = open({ site_access: "private", site_url });
    flush(() => box.click());
    await until(() => note().startsWith("Only"));
    expect(sent[1].body).toEqual({ site_access: "private" });
    m.unmount();
    // A daemon that does not say offers no toggle.
    const old = mount(ShareDialog, { id: ID, title: "Plan", initial: open(), onClose: vi.fn() });
    expect(old.root.querySelector("input[name=world]")).toBeNull();
    old.unmount();
  });

  it("says what the daemon refused", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ error: { code: "forbidden", message: "only the page's maker shares it" } }), { status: 403 })));
    const m = mount(ShareDialog, { id: ID, title: "Plan", initial: open(), onClose: vi.fn() });
    const who = m.root.querySelector<HTMLInputElement>("input[name=who]")!;
    flush(() => { who.value = "octo"; who.dispatchEvent(new Event("input", { bubbles: true })); });
    flush(() => m.root.querySelector<HTMLFormElement>("form.add")!.requestSubmit());
    const err = await until(() => m.root.querySelector("[role=alert]"));
    expect(err.textContent).toBe("only the page's maker shares it");
    m.unmount();
  });

  it("offers no Public while the public gallery is off", () => {
    const state = { sharing: open({ public_gallery: false }) as Sharing };
    serve(state);
    const m = mount(ShareDialog, { id: ID, title: "Plan", initial: open({ public_gallery: false }), onClose: vi.fn() });
    const general = m.root.querySelector<HTMLSelectElement>("select[aria-label='General access']")!;
    expect([...general.options].map(o => o.textContent)).toEqual(["Restricted", "Anyone with the link"]);
    m.unmount();
  });
});
