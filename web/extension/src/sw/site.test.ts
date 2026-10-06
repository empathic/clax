import { describe, expect, it } from "vitest";
import type { HubMsg, TabMsg } from "../../../shell/src/stream-hub";
import type { PageView, SiteView } from "../messages";
import { Sites, applySite } from "./site";

const O = "http://localhost:5173";
const A1 = "7q3k9mzx2b4t";
const A2 = "8r4m0nzy3c5v";
const A3 = "9s5n1pza4d6w";
const T1 = "01J9AAAAAAAAAAAAAAAAAAAAAA";
const T2 = "01J9BBBBBBBBBBBBBBBBBBBBBB";
const page = (aid: string, path: string): PageView => ({ artifact_id: aid, origin: O, path, page_url: O + path, title: path, current_version: 1, url: `http://127.0.0.1:7481/a/${aid}` });
const comment = (tid: string, n: number) => ({ id: `c${tid}${n}`, thread_id: tid, author_kind: "viewer", author_name: "Alex", via_harness: null, body: `body ${n}`, created_at: "t" });
const thread = (id: string, aid: string, n = 1) => ({ id, artifact_id: aid, status: "open", anchor: { kind: "element", selector: "h1", file: "index.html" }, comments: Array.from({ length: n }, (_, k) => comment(id, k + 1)), feedback_state: null, page_path: "/a" });
const delta = (id: string, aid: string, count: number) => {
  const { comments, ...rest } = thread(id, aid, count);
  return { ...rest, comment_count: count, last_comment: comments.at(-1) };
};
const site = (): SiteView => ({ origin: O, rules: [], pages: [{ page: page(A1, "/a"), threads: [thread(T1, A1) as never] }, { page: page(A2, "/b"), threads: [thread(T2, A2) as never] }] });

describe("applySite", () => {
  it("adds a comment to its thread, and asks for the listing again when the comments do not add up or the page is new", () => {
    const v = applySite(site(), "thread", { artifact_id: A1, thread: delta(T1, A1, 2) })!;
    expect(v.pages[0].threads[0].comments.map(c => c.body)).toEqual(["body 1", "body 2"]);
    expect(applySite(site(), "thread", { artifact_id: A1, thread: delta(T1, A1, 3) })).toBeNull();
    expect(applySite(site(), "thread", { artifact_id: A3, thread: delta(T1, A3, 1) })).toBeNull();
  });

  it("moves a moved thread's card to the page it went to, comments and all", () => {
    const v = applySite(site(), "thread_moved", { artifact_id: A1, thread_id: T1, to_artifact_id: A2 })!;
    expect(v.pages[0].threads).toEqual([]);
    expect(v.pages[1].threads.map(t => t.id)).toEqual([T2, T1]);
    expect(v.pages[1].threads[1]).toMatchObject({ artifact_id: A2, comments: [{ body: "body 1" }] });
    // Its `thread` on the new page then completes it.
    const w = applySite(v, "thread", { artifact_id: A2, thread: { ...delta(T1, A2, 1), page_path: "/b" } })!;
    expect(w.pages[1].threads[1]).toMatchObject({ page_path: "/b", comments: [{ body: "body 1" }] });
    // A page the listing does not hold yet: the listing is fetched again.
    expect(applySite(site(), "thread_moved", { artifact_id: A1, thread_id: T1, to_artifact_id: A3 })).toBeNull();
  });

  it("drops deleted threads, fetches the listing again for a deleted page, and leaves it alone for anything else", () => {
    expect(applySite(site(), "thread_deleted", { artifact_id: A1, thread_id: T1 })!.pages[0].threads).toEqual([]);
    expect(applySite(site(), "artifact_deleted", { artifact_id: A1 })).toBeNull();
    const v = site();
    expect(applySite(v, "version", { artifact_id: A1, n: 4 })).toBe(v);
    expect(applySite(v, "feedback_state", { artifact_id: A2, thread_id: T2, state: "sent" })).toBe(v);
  });
});

function harness() {
  const hubIn: { id: string; msg: TabMsg }[] = [];
  const detached: string[] = [];
  const changed: string[] = [];
  const loads: string[] = [];
  let answer: SiteView = site();
  let gate: Promise<void> | null = null;
  const sites = new Sites({
    api: { site: async (o: string) => { loads.push(o); const v = structuredClone(answer); await gate; return v; } },
    hub: { receive: (id, msg) => hubIn.push({ id, msg }), detach: id => detached.push(id) },
    changed: o => changed.push(o),
  });
  const hear = (msg: HubMsg) => sites.fromHub(`site:${O}`, msg);
  return { sites, hubIn, detached, changed, loads, hear, set: (v: SiteView) => { answer = v; }, hold: (p: Promise<void> | null) => { gate = p; } };
}
const tick = () => new Promise(r => setTimeout(r, 0));

describe("Sites", () => {
  it("follows the site topic of each origin a tab is on for, and drops it after", () => {
    const h = harness();
    h.sites.follow([O, O]);
    expect(h.hubIn).toEqual([{ id: `site:${O}`, msg: { t: "topics", topics: [`site:${O}`] } }]);
    h.sites.follow([]);
    expect(h.detached).toEqual([`site:${O}`]);
    expect(h.sites.view(O)).toBeNull();
  });

  it("loads the listing when the topic goes live, applies the deltas, and fetches it again when one does not add up", async () => {
    const h = harness();
    h.sites.follow([O]);
    h.hear({ t: "live", topics: [`site:${O}`] });
    await tick();
    expect(h.loads).toEqual([O]);
    expect(h.sites.view(O)?.pages).toHaveLength(2);
    expect(h.changed).toEqual([O]);
    h.hear({ t: "event", topic: `site:${O}`, name: "thread_moved", data: { artifact_id: A1, thread_id: T1, to_artifact_id: A2 } });
    expect(h.sites.view(O)?.pages[1].threads).toHaveLength(2);
    expect(h.changed).toHaveLength(2);
    h.hear({ t: "event", topic: `site:${O}`, name: "thread", data: { artifact_id: A3, thread: delta(T1, A3, 1) } });
    await tick();
    expect(h.loads).toHaveLength(2);
    h.hear({ t: "ping" });
    expect(h.hubIn.at(-1)).toEqual({ id: `site:${O}`, msg: { t: "ping" } });
  });

  it("applies again, to a listing that answers late, the deltas heard while it was asked for", async () => {
    const h = harness();
    h.sites.follow([O]);
    let open!: () => void;
    h.hold(new Promise<void>(r => { open = r; }));
    h.hear({ t: "resync", topic: `site:${O}` });
    await tick();
    h.hear({ t: "event", topic: `site:${O}`, name: "thread_deleted", data: { artifact_id: A2, thread_id: T2 } });
    open();
    await tick();
    expect(h.sites.view(O)?.pages[1].threads).toEqual([]);
  });

  it("drops a listing that answers after its origin was let go", async () => {
    const h = harness();
    h.sites.follow([O]);
    let open!: () => void;
    h.hold(new Promise<void>(r => { open = r; }));
    const loading = h.sites.load(O);
    h.sites.follow([]);
    open();
    await loading;
    expect(h.sites.view(O)).toBeNull();
    expect(h.changed).toEqual([]);
  });
});
