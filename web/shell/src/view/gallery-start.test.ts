import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { ApiError } from "../trouble";
import type { GalleryBoot } from "./gallery-boot";
import { type Answer, type Io, contentFromBlock, gatherContent, gatherHeader, header, headerFromBlock, pageKind } from "./gallery-start";

const VIEWER = { viewer: { public_id: "v1", display_name: "Ada" }, may_create: false };
const PERSON = { viewer: { public_id: "p1", display_name: "Sam", person: true, person_revoked_at: null }, may_create: true };
const OWNER = { viewer: { public_id: "o1", display_name: "Alex", owner: true }, may_create: true };
const SUMMARY = { unread: 2, questions: [], latest: [] };
const ok = <T>(body: T): Answer<T> => ({ status: 200, body });
const no = (status: number): Answer<never> => ({ status, body: null });

/** A whole block for `viewer`, as the daemon writes it on `/`. */
function block(viewer: unknown, inbox: boolean | null, extra: Partial<GalleryBoot> = {}): GalleryBoot {
  return {
    v: 1, kind: "gallery",
    status: { viewer: 200, hosted: 404, feedback: 200, bot: 404, summary: inbox ? 200 : 403, artifacts: 200, attention: 200, questions: inbox ? 200 : 403 },
    viewer: viewer as GalleryBoot["viewer"], hosted: null, feedback: { enabled: false }, bot: null, inbox,
    summary: inbox ? SUMMARY : null,
    artifacts: { artifacts: [{ id: "a1", title: "A", description: null, icon: null, updated_at: "2026-11-01T00:00:00Z", current_version: 1, pinned: false }] },
    attention: { artifacts: { a1: { addressed: [], addressed_v: null, new_replies: [], open_in: [], seen: 1 } } },
    questions: inbox ? { questions: [], open: 0 } : null,
    ...extra,
  };
}

/** An `Io` answering `routes` by path (404 for any other), keeping the paths asked in order. */
function io(routes: Record<string, Answer<unknown>>, opts: { token?: string | null | "failed"; proof?: boolean } = {}) {
  const asked: string[] = [];
  const answer = (path: string) => { asked.push(path); return Promise.resolve(routes[path] ?? no(404)); };
  const x: Io = {
    get: answer,
    token: () => { asked.push("/api/token"); return Promise.resolve(opts.token === undefined ? null : opts.token); },
    feedback: () => answer("/_app/feedback/config").then(a => a.body),
    list: () => answer("/api/artifacts").then(a => (a.body as { artifacts: never[] }).artifacts),
    proof: !!opts.proof,
  };
  return { io: x, asked };
}

describe("pageKind", () => {
  // The entry's inline script, run against a path: the kind it marks the root with.
  const script = /<script id="clax-page">([\s\S]*?)<\/script>/.exec(readFileSync(join(__dirname, "../../index.html"), "utf8"))![1];
  const inline = (path: string) => {
    const root = { dataset: {} as Record<string, string> };
    const doc = { documentElement: root, getElementById: () => null, createElement: () => ({}), head: { appendChild() {} } };
    new Function("document", "location", "localStorage", script)(doc, { pathname: path }, { length: 0, key: () => null });
    return root.dataset.page;
  };
  it("tells the gallery, the pages beside it and the sign-in pages apart, as the entry's inline script does", () => {
    const cases: [string, string][] = [
      ["/", "gallery"], ["/a/nothing", "gallery"], ["/inbox", "page"], ["/tags", "page"], ["/queues", "page"], ["/queues/q1/t1", "page"],
      ["/settings", "page"], ["/settings/people", "page"], ["/people", "page"], ["/setup", "auth"], ["/device", "auth"],
      ["/join/abc", "auth"], ["/signin", "auth"], ["/signin/xyz", "auth"], ["/start/", "auth"],
    ];
    for (const [path, kind] of cases) {
      expect(pageKind(path), path).toBe(kind);
      expect(inline(path), path).toBe(kind);
    }
  });
});

describe("header", () => {
  it("is the owner's for the token, a hosted owner's session, or the owner's viewer", () => {
    expect(header({ viewer: ok(VIEWER), token: "t" })).toMatchObject({ owner: true, member: true, queues: true, inbox: true, site: false });
    expect(header({ viewer: ok(VIEWER), token: "cxo_1" })).toMatchObject({ owner: true, site: true });
    const hosted = { origin: "https://c.example", setup_open: false, signed_in: true, person: { public_id: "o1", display_name: "Ada", owner: true, guest: false } };
    expect(header({ viewer: ok(VIEWER), hosted: ok(hosted), token: null })).toMatchObject({ owner: true, member: true, queues: true, site: true });
    expect(header({ viewer: ok(OWNER) }, true)).toMatchObject({ owner: true, inbox: true });
  });

  it("gives a hosted guest no gear, and a LAN viewer no inbox once the token is refused", () => {
    const guest = { origin: "https://c.example", setup_open: false, signed_in: true, person: { public_id: "g1", display_name: "Mia", owner: false, guest: true } };
    expect(header({ viewer: ok(VIEWER), hosted: ok(guest), token: null })).toMatchObject({ owner: false, member: false, queues: false, inbox: false, site: true });
    expect(header({ viewer: ok(VIEWER), hosted: no(404), token: null })).toMatchObject({ member: false, inbox: false });
    // A token route that did not answer says nothing: the owner check asks again.
    expect(header({ viewer: ok(VIEWER), token: "failed" }).inbox).toBeNull();
  });

  it("shows Queues to the owner and the instance's admins, and not to a member", () => {
    expect(header({ viewer: ok(PERSON), bot: ok({ enabled: false, role: "admin" }), token: null })).toMatchObject({ member: true, queues: true, inbox: true });
    expect(header({ viewer: ok(PERSON), bot: ok({ enabled: false, role: "member" }), token: null })).toMatchObject({ member: true, queues: false });
    expect(header({ viewer: ok(PERSON), bot: no(404), token: null }).queues).toBe(false);
    // A revoked person is no member.
    expect(header({ viewer: ok({ viewer: { ...PERSON.viewer, person_revoked_at: "2026-10-12T10:00:00Z" } }), token: null })).toMatchObject({ member: false, inbox: false });
  });
});

describe("the block", () => {
  it("gives the header and the content at once when it holds every answer", () => {
    const b = block(VIEWER, false);
    const h = headerFromBlock(b)!;
    expect(h).toMatchObject({ owner: false, member: false, inbox: false, summary: null });
    expect(contentFromBlock(b, h)).toMatchObject({ artifacts: b.artifacts!.artifacts, error: null, attention: b.attention!.artifacts, questions: null });
    const o = block(OWNER, true);
    const oh = headerFromBlock(o)!;
    expect(oh).toMatchObject({ owner: true, inbox: true, summary: SUMMARY });
    expect(contentFromBlock(o, oh)!.questions).toEqual([]);
  });

  it("leaves to the routes what it lacks: a field it has no status for, or an inbox it cannot tell", () => {
    expect(headerFromBlock(block(VIEWER, false, { status: { hosted: 404, feedback: 200, bot: 404 } }))).toBeNull();
    expect(headerFromBlock(block(VIEWER, null))).toBeNull();
    const b = block(OWNER, true, { status: { viewer: 200, hosted: 404, feedback: 200, bot: 404, summary: 200, artifacts: 200, attention: 200 } });
    expect(contentFromBlock(b, headerFromBlock(b))).toBeNull();
  });
});

describe("gathering without a block", () => {
  it("asks the header's routes and the content's at once, and an owner's inbox once the token says so", async () => {
    const { io: x, asked } = io({ "/api/viewers/me": ok(VIEWER), "/api/artifacts": ok({ artifacts: [] }), "/api/inbox/summary": ok(SUMMARY), "/api/questions?status=open&limit=200": ok({ questions: [], open: 0 }) }, { token: "t" });
    const t = x.token();
    const h = gatherHeader(null, x, t);
    const c = gatherContent(null, x, h);
    // Before any answer is taken: everything that does not wait on who the browser is.
    expect(asked.sort()).toEqual(["/_app/feedback/config", "/api/artifacts", "/api/hosted/me", "/api/me/bot", "/api/token", "/api/viewers/me", "/api/viewers/me/attention"]);
    expect(await h).toMatchObject({ owner: true, inbox: true, summary: SUMMARY });
    expect((await c).questions).toEqual([]);
    expect(asked).toContain("/api/inbox/summary");
  });

  it("asks a browser holding the owner's proof for its inbox at once, beside the rest", () => {
    const { io: x, asked } = io({}, { token: "t", proof: true });
    const h = gatherHeader(null, x, x.token());
    void gatherContent(null, x, h);
    expect(asked).toContain("/api/inbox/summary");
    expect(asked).toContain("/api/questions?status=open&limit=200");
  });

  it("asks nothing of an inbox for a browser with none", async () => {
    const { io: x, asked } = io({ "/api/viewers/me": ok(VIEWER), "/api/artifacts": ok({ artifacts: [] }) }, { token: null });
    const h = gatherHeader(null, x, x.token());
    const c = await gatherContent(null, x, h);
    expect((await h).inbox).toBe(false);
    expect(c.questions).toBeNull();
    expect(asked.some(p => /^\/api\/(inbox|questions)/.test(p))).toBe(false);
  });

  it("takes what a block holds and asks for the rest", async () => {
    const b = block(PERSON, true, { status: { viewer: 200, hosted: 404, feedback: 200, bot: 404, artifacts: 200, attention: 200 } });
    const { io: x, asked } = io({ "/api/inbox/summary": ok(SUMMARY), "/api/questions?status=open&limit=200": ok({ questions: [], open: 0 }) });
    const h = await gatherHeader(b, x, x.token());
    const c = await gatherContent(b, x, Promise.resolve(h));
    expect(h).toMatchObject({ member: true, inbox: true, summary: SUMMARY });
    expect(c.artifacts).toEqual(b.artifacts!.artifacts);
    expect(asked.filter(p => p !== "/api/token").sort()).toEqual(["/api/inbox/summary", "/api/questions?status=open&limit=200"]);
  });

  it("says why the list did not load", async () => {
    const { io: x } = io({});
    x.list = () => Promise.reject(new ApiError(403, "Only the owner may list artifacts", "forbidden"));
    const c = await gatherContent(null, x, gatherHeader(null, x, x.token()));
    expect(c).toMatchObject({ artifacts: null, error: "Only the owner may list artifacts", attention: {} });
  });

  it("says a list no Clax answered, and a block's refused list, without a status number", async () => {
    const { io: x } = io({});
    x.list = () => Promise.reject(new ApiError(530, "error code: 1016", null, false));
    const c = await gatherContent(null, x, gatherHeader(null, x, x.token()));
    expect(c.error).toBe("this Clax is not answering. It may have been taken down or moved.");
    const b = block(PERSON, true, { status: { viewer: 200, hosted: 404, feedback: 200, bot: 404, artifacts: 401, attention: 200 } });
    const d = await gatherContent(b, x, gatherHeader(b, x, x.token()));
    expect(d).toMatchObject({ artifacts: null, error: "you are signed out." });
  });
});
