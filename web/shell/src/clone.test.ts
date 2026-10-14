import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "./test/svelte";
import Clone from "./ui/Clone.svelte";
import CloneDialog from "./ui/CloneDialog.svelte";
import MoreMenu from "./ui/MoreMenu.svelte";
import { expiresIn } from "./ui/clone-api";
import { cloneOpen } from "./ui/more-menu.svelte";

const ID = "7q3k9mzx2b4t";
const REMOTE = `http://clax.test/git/a-${ID}.git`;
const header = (t: string) => `http.extraHeader="Authorization: Bearer ${t}"`;
const token = (scope: "read" | "write") => {
  const t = `cxm_${scope}_abc`;
  return {
    artifact_id: ID, scope, remote: REMOTE, token: t, expires_at: new Date(Date.now() + 3600_000).toISOString(),
    clone_command: `git -c ${header(t)} clone ${REMOTE}`,
    jj_clone_command: `GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=http.extraHeader GIT_CONFIG_VALUE_0="Authorization: Bearer ${t}" jj git clone ${REMOTE}`,
    existing_checkout: {
      remote_name: "clax", add_remote_command: `git remote add clax ${REMOTE}`, fetch_command: `git -c ${header(t)} fetch clax`,
      ...(scope === "write" ? { push_command: `git -c ${header(t)} push clax HEAD:main` } : {}),
      jj_add_remote_command: `jj git remote add clax ${REMOTE}`, jj_fetch_command: `… jj git fetch --remote clax`,
    },
  };
};

/** `fetch` answering the token request and the git token routes from `options`. */
function serve(options: unknown, posted: string[] = []) {
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    if (url === "/api/token") return new Response("{}", { status: 403 });
    if (url === `/api/artifacts/${ID}/git-token` && (init?.method ?? "GET") === "GET") return new Response(JSON.stringify(options));
    if (url === `/api/artifacts/${ID}/git-token` && init?.method === "POST") {
      const scope = JSON.parse(String(init.body)).scope;
      posted.push(scope);
      if (scope === "write") return new Response(JSON.stringify({ error: { code: "not_permitted", message: "your rights here do not let you push to this artifact" } }), { status: 403 });
      return new Response(JSON.stringify(token(scope)));
    }
    return new Response("{}", { status: 404 });
  }));
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

describe("Clone", () => {
  it("shows nothing where Clax keeps no repositories, or for a viewer with no scope", async () => {
    serve({ available: false, scopes: [] });
    const m = mount(Clone, { id: ID });
    await new Promise(r => setTimeout(r, 20));
    flush();
    expect(m.root.querySelector("button.clone")).toBeNull();
    expect(cloneOpen.current).toBeNull();
    m.unmount();
    serve({ available: true, scopes: [], reason: { code: "sign_in_required", message: "…" } });
    const g = mount(Clone, { id: ID });
    await new Promise(r => setTimeout(r, 20));
    flush();
    expect(g.root.querySelector("button.clone")).toBeNull();
    g.unmount();
  });

  it("shows Clone where the viewer may clone, offers the more menu's item, and opens the dialog", async () => {
    serve({ available: true, scopes: ["read"], ttl_s: 3600 });
    const m = mount(Clone, { id: ID });
    const b = await until(() => m.root.querySelector<HTMLButtonElement>("button.clone"));
    expect(b.classList.contains("hide-sm")).toBe(true);
    expect(cloneOpen.current).not.toBeNull();
    flush(() => b.click());
    const dialog = await until(() => document.querySelector("[role=dialog]"));
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(dialog.querySelector("h2")?.textContent).toBe("Clone with git or jj");
    m.unmount();
    expect(cloneOpen.current).toBeNull();
  });
});

describe("Clone dialog", () => {
  it("offers the scopes the rights allow, makes a token and shows the git and jj commands with copy buttons", async () => {
    const posted: string[] = [];
    serve(null, posted);
    const writes: string[] = [];
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText: async (t: string) => { writes.push(t); } } });
    const onClose = vi.fn();
    const m = mount(CloneDialog, { id: ID, options: { available: true, scopes: ["read", "write"], ttl_s: 3600 }, onClose });
    const radios = Array.from(m.root.querySelectorAll<HTMLInputElement>("input[type=radio]"));
    expect(radios.map(r => r.value)).toEqual(["read", "write"]);
    expect(radios[0].checked).toBe(true);
    expect(m.root.textContent).toContain("v1, v2 and so on. A push to main becomes a new version, published by you.");
    expect(m.root.textContent).toContain("A token lives an hour and is shown once.");
    const make = Array.from(m.root.querySelectorAll<HTMLButtonElement>("button")).find(b => b.textContent === "Make a token")!;
    flush(() => make.click());
    const pres = await until(() => { const p = m.root.querySelectorAll("pre"); return p.length === 4 && p; });
    expect(posted).toEqual(["read"]);
    expect(document.activeElement?.getAttribute("aria-label"), "focus moves to the first Copy").toBe("Copy git");
    const t = token("read");
    expect(pres[0].textContent).toBe(t.clone_command);
    expect(pres[1].textContent).toBe(t.jj_clone_command);
    expect(pres[2].textContent).toBe(`git remote add clax ${REMOTE}\n${t.existing_checkout.fetch_command}`);
    expect(m.root.textContent).toMatch(/The token expires in about (59|60) min\. Until then anyone holding it can clone this artifact\./);
    const copyGit = m.root.querySelector<HTMLButtonElement>("button[aria-label='Copy git']")!;
    flush(() => copyGit.click());
    await until(() => copyGit.textContent === "Copied");
    expect(writes).toEqual([t.clone_command]);
    // A refused scope says why.
    flush(() => { radios[1].click(); });
    const again = Array.from(m.root.querySelectorAll<HTMLButtonElement>("button")).find(b => b.textContent === "Make a new token")!;
    flush(() => again.click());
    const err = await until(() => m.root.querySelector("[role=alert]"));
    expect(err.textContent).toBe("your rights here do not let you push to this artifact");
    // Escape closes it.
    m.root.querySelector<HTMLElement>("[role=dialog]")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(onClose).toHaveBeenCalledTimes(1);
    m.unmount();
  });

  it("offers read alone to a viewer who may not push", () => {
    serve(null);
    const m = mount(CloneDialog, { id: ID, options: { available: true, scopes: ["read"], ttl_s: 3600 }, onClose: vi.fn() });
    expect(Array.from(m.root.querySelectorAll<HTMLInputElement>("input[type=radio]")).map(r => r.value)).toEqual(["read"]);
    expect(m.root.textContent).not.toContain("A push to main");
    m.unmount();
  });

  it("tells whoever reads a page through its link that its token is read-only and short-lived", () => {
    serve(null);
    const m = mount(CloneDialog, { id: ID, options: { available: true, scopes: ["read"], ttl_s: 900, link: true }, onClose: vi.fn() });
    expect(Array.from(m.root.querySelectorAll<HTMLInputElement>("input[type=radio]")).map(r => r.value)).toEqual(["read"]);
    expect(m.root.textContent).toContain("This page's link lets you clone it read-only; pushing needs a member's rights.");
    expect(m.root.textContent).toContain("A token lives 15 minutes and is shown once.");
    m.unmount();
  });

  it("shows Clone to a link's reader", async () => {
    serve({ available: true, scopes: ["read"], ttl_s: 900, link: true });
    const m = mount(Clone, { id: ID });
    await until(() => m.root.querySelector<HTMLButtonElement>("button.clone"));
    m.unmount();
  });

  it("says when a token expires", () => {
    const now = Date.parse("2026-10-13T12:00:00Z");
    expect(expiresIn("2026-10-13T13:00:00Z", now)).toBe("in about 60 min");
    expect(expiresIn("2026-10-13T15:00:00Z", now)).toBe("in about 3 h");
    expect(expiresIn("2026-10-13T11:00:00Z", now)).toBe("now");
  });
});

describe("the more menu at phone width", () => {
  it("lists Clone with git when the viewer may clone, which closes it and opens the dialog", async () => {
    vi.stubGlobal("matchMedia", (q: string) => ({ matches: q === "(max-width: 700px)" }));
    const onClone = vi.fn();
    const m = mount(MoreMenu, { rawHref: "/c/x/v/1/", canCopy: true, onCopy: vi.fn(), onVersions: vi.fn(), onClone });
    flush(() => m.root.querySelector<HTMLButtonElement>("button.icon")!.click());
    await new Promise(r => setTimeout(r));
    const items = Array.from(m.root.querySelectorAll<HTMLElement>("[role=menuitem]"));
    expect(items.map(i => i.textContent)).toEqual(["Versions", "Open raw", "Copy link", "Clone with git…"]);
    flush(() => items[3].click());
    expect(onClone).toHaveBeenCalledTimes(1);
    expect(m.root.querySelector("[role=menu]")).toBeNull();
    m.unmount();
  });
});
