import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ArtifactController, ViewState } from "./artifact-controller";
import { SKELETON_HTML } from "./skeleton";
import { Store } from "./store";
import { FIELD_LABEL, RENAME_FAILED, editTitle, followTitle, titleOf } from "./title-live";

type Art = { title: string; untitled?: boolean; may_edit?: boolean; live?: unknown };

/** A view showing `a`: the controller parts `followTitle` uses, over a
 * real store, with the view's own heading writer (`pageFollows` sets the
 * heading's text as the title changes). */
function view(a: Art) {
  document.body.innerHTML = `<div id="app"><div class="page">${SKELETON_HTML}</div></div>`;
  const h1 = document.querySelector<HTMLElement>(".topbar .ttl > h1")!;
  const state = new Store({ error: null, deleted: false, data: { artifact: { id: "a1", ...a }, versions: [] } } as unknown as ViewState);
  state.subscribe(s => { const t = s.data!.artifact.title; if (h1.textContent !== t) h1.textContent = t; });
  const notices: (string | null)[] = [];
  const c = { id: "a1", disposed: false, state, set: (p: Parameters<Store<ViewState>["set"]>[0]) => state.set(p), setNotice: (n: string | null) => { notices.push(n); } } as unknown as ArtifactController;
  return { c, h1, state, notices };
}

const key = (el: Element, k: string) => el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));

/** A daemon answering PATCH /api/artifacts/a1 with `answer`, recording the bodies sent. */
function daemon(answer: (body: { title: string }) => [number, unknown]) {
  const sent: { title: string }[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    if (url === "/api/token") return new Response("{}", { status: 403 });
    expect(url).toBe("/api/artifacts/a1");
    expect(init?.method).toBe("PATCH");
    const body = JSON.parse(String(init?.body));
    sent.push(body);
    const [status, json] = answer(body);
    return new Response(JSON.stringify(json), { status, headers: { "content-type": "application/json" } });
  }));
  return sent;
}

beforeEach(() => { document.title = "(3) Clax"; });
afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); document.getElementById("ttl-css")?.remove(); });

describe("titleOf", () => {
  it("says the title, the artifact's own (none while untitled), and whether the viewer may rename it", () => {
    const s = (a: Art, more: Partial<ViewState> = {}) => ({ error: null, deleted: false, data: { artifact: a }, ...more }) as unknown as ViewState;
    expect(titleOf(s({ title: "Plan", may_edit: true }))).toEqual({ title: "Plan", own: "Plan", may: true });
    expect(titleOf(s({ title: "Untitled", untitled: true }))).toEqual({ title: "Untitled", own: "", may: false });
    expect(titleOf(s({ title: "Plan", may_edit: true }, { deleted: true })).may).toBe(false);
    expect(titleOf(s({ title: "Page", may_edit: true, live: {} })).may).toBe(false);
    expect(titleOf({ error: "gone", data: null } as unknown as ViewState).title).toBeNull();
  });
});

describe("followTitle", () => {
  it("keeps the tab on the title, after the unread count", () => {
    const { c, state } = view({ title: "Plan" });
    followTitle(c);
    expect(document.title).toBe("(3) Plan · Clax");
    state.set(s => ({ data: { ...s.data!, artifact: { ...s.data!.artifact, title: "Renamed" } } }));
    expect(document.title).toBe("(3) Renamed · Clax");
  });

  it("leaves the heading plain text for a viewer who may not rename", () => {
    const { c, h1 } = view({ title: "Plan", may_edit: false });
    followTitle(c);
    expect(h1.querySelector("button")).toBeNull();
    expect(h1.textContent).toBe("Plan");
  });

  it("renames in place: Enter saves, the heading and tab follow, and focus returns to the title", async () => {
    const { c, h1 } = view({ title: "Plan", may_edit: true });
    const sent = daemon(b => [200, { artifact: { id: "a1", title: b.title, description: null, icon: null } }]);
    followTitle(c);
    const b = h1.querySelector<HTMLButtonElement>("button.ttl-name")!;
    expect(b.textContent).toBe("Plan");
    expect(document.getElementById(b.getAttribute("aria-describedby")!)!.textContent).toBe("Rename");
    b.click();
    const input = document.querySelector<HTMLInputElement>(".ttl > input.ttl-edit")!;
    expect(input.getAttribute("aria-label")).toBe(FIELD_LABEL);
    expect(h1.hidden).toBe(true);
    expect(document.activeElement).toBe(input);
    expect(input.value).toBe("Plan");
    input.value = "  Q3 plan ";
    key(input, "Enter");
    expect(document.querySelector(".ttl-edit")).toBeNull();
    expect(h1.hidden).toBe(false);
    await vi.waitFor(() => expect(h1.textContent).toBe("Q3 plan"));
    expect(sent).toEqual([{ title: "Q3 plan" }]);
    expect(document.title).toBe("(3) Q3 plan · Clax");
    expect(document.activeElement).toBe(h1.querySelector("button.ttl-name"));
  });

  it("Escape cancels, and an unchanged title sends nothing", async () => {
    const { c, h1 } = view({ title: "Plan", may_edit: true });
    const sent = daemon(() => [500, {}]);
    followTitle(c);
    h1.querySelector<HTMLButtonElement>("button")!.click();
    const input = document.querySelector<HTMLInputElement>(".ttl-edit")!;
    input.value = "Other";
    key(input, "Escape");
    h1.querySelector<HTMLButtonElement>("button")!.click();
    document.querySelector<HTMLInputElement>(".ttl-edit")!.blur();
    expect(document.querySelector(".ttl-edit")).toBeNull();
    expect(sent).toEqual([]);
    expect(h1.textContent).toBe("Plan");
  });

  it("an untitled artifact's field starts empty, and leaving an empty one saves it untitled", async () => {
    const { c, h1 } = view({ title: "Untitled", untitled: true, may_edit: true });
    const sent = daemon(b => [200, { artifact: { id: "a1", title: b.title || "Untitled", untitled: !b.title, description: null, icon: null } }]);
    followTitle(c);
    h1.querySelector<HTMLButtonElement>("button")!.click();
    let input = document.querySelector<HTMLInputElement>(".ttl-edit")!;
    expect(input.value).toBe("");
    expect(input.placeholder).toBe("Untitled");
    input.value = "Named";
    input.blur();
    await vi.waitFor(() => expect(h1.textContent).toBe("Named"));
    h1.querySelector<HTMLButtonElement>("button")!.click();
    input = document.querySelector<HTMLInputElement>(".ttl-edit")!;
    input.value = "";
    input.blur();
    await vi.waitFor(() => expect(h1.textContent).toBe("Untitled"));
    expect(sent).toEqual([{ title: "Named" }, { title: "" }]);
  });

  it("a refused rename says why and keeps the title", async () => {
    const { c, h1, notices } = view({ title: "Plan", may_edit: true });
    daemon(() => [400, { error: { code: "title_too_long", message: "title exceeds 200 characters" } }]);
    followTitle(c);
    h1.querySelector<HTMLButtonElement>("button")!.click();
    const input = document.querySelector<HTMLInputElement>(".ttl-edit")!;
    input.value = "x";
    key(input, "Enter");
    await vi.waitFor(() => expect(notices).toEqual([`${RENAME_FAILED}: title exceeds 200 characters.`]));
    expect(h1.textContent).toBe("Plan");
  });

  it("stops: the heading is plain text again", () => {
    const { c, h1 } = view({ title: "Plan", may_edit: true });
    const stop = followTitle(c);
    expect(h1.querySelector("button")).not.toBeNull();
    stop();
    expect(h1.querySelector("button")).toBeNull();
    expect(h1.textContent).toBe("Plan");
  });
});

describe("editTitle", () => {
  it("keeps its keys from the view's shortcuts", () => {
    document.body.innerHTML = `<div class="ttl"><h1>Plan</h1></div>`;
    const h1 = document.querySelector("h1")!;
    const heard = vi.fn();
    document.body.addEventListener("keydown", heard);
    const input = editTitle(h1, "Plan", "Plan", vi.fn(), vi.fn());
    key(input, "c");
    expect(heard).not.toHaveBeenCalled();
  });
});
