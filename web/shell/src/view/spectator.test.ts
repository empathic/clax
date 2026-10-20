import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeWorker, artifactStreams } from "../test/fake-worker";
import { MOUNT_TIMEOUT_MS, WAIT_MS } from "../test/timeouts";
import type { Thread } from "../threads";
import { INPUT_JOIN_GAP_MS, SNAPSHOT_POLL_MS, joinOffered } from "./spectator";

const ID = "7q3k9mzx2b4t";
const FakeES = artifactStreams(ID);
const artifact = (n: number) => ({ id: ID, title: "T", description: null, icon: null, updated_at: "x", current_version: n, pinned: false, working: [] });
const version = (n: number) => ({ artifact_id: ID, n, label: null, created_at: "x", files: {} });
const thread = (id: string): Thread => ({
  id, artifact_id: ID, version_n: 1, status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "2026-09-30T10:00:00.000Z",
  resolved_at: null, resolved_by: null, feedback_state: null, comments: [{ id: `${id}c`, thread_id: id, author_kind: "viewer", author_name: "alex", via_harness: null, body: "x", created_at: "2026-09-30T10:00:00.000Z" }],
  anchor: { kind: "element", selector: "h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
});

/** What the snapshot route answers next. */
let snap: Record<string, unknown>;
let snapshots = 0;

/** The controllers a test started: disposed after it, even when it fails part-way, so none leaves its timers running. */
const controllers: { dispose(): void }[] = [];

async function started() {
  vi.stubGlobal("SharedWorker", FakeWorker);
  vi.stubGlobal("fetch", vi.fn(async (url: string) => {
    if (url.startsWith(`/api/artifacts/${ID}/snapshot`)) { snapshots++; return new Response(JSON.stringify(snap)); }
    return new Response(JSON.stringify(
      url.includes("/threads") ? { threads: [], next_cursor: null }
      : url.startsWith("/api/viewers") ? { viewer: { public_id: "u_1", display_name: "Pat", created_at: "x", person: true } }
      : url === "/api/token" ? { error: { code: "not_loopback", message: "not a loopback connection" } }
      : { artifact: artifact(1), versions: [version(1)] }), { status: url === "/api/token" ? 403 : 200 });
  }));
  sessionStorage.setItem("clax.origin-ok", "0");
  const { ArtifactController } = await import("./artifact-controller");
  const { FrameHost } = await import("./frame-host");
  const stage = document.createElement("div");
  document.body.append(stage);
  const ctl = new ArtifactController({ id: ID, pinnedVersion: null });
  controllers.push(ctl);
  ctl.frame = new FrameHost(stage, () => ctl.frameLoaded());
  ctl.start();
  const deadline = Date.now() + WAIT_MS;
  while (!stage.querySelector("iframe") && Date.now() < deadline) await new Promise(r => setTimeout(r, 5));
  stage.querySelector("iframe")!.dispatchEvent(new Event("load"));
  await vi.waitFor(() => expect(FakeES.last).toBeDefined());
  return { ctl, w: FakeES.last!.w };
}

const retries = (w: FakeWorker) => w.msgs.filter(m => m.t === "retry");

describe("spectator mode", { timeout: MOUNT_TIMEOUT_MS }, () => {
  beforeEach(() => {
    vi.resetModules();
    FakeES.last = undefined;
    history.replaceState(null, "", `/a/${ID}`);
    snapshots = 0;
    snap = { artifact: artifact(1), versions: [version(1)], threads: [], presence: [], viewing: 2, seats_free: 0, guest_seats_free: 0, built_at: 1 };
  });
  afterEach(async () => { for (const c of controllers.splice(0)) c.dispose(); (await import("../caps/gesture")).unwatchShell(); vi.unstubAllGlobals(); sessionStorage.clear(); document.body.replaceChildren(); vi.useRealTimers(); });

  it("offers Join live from the seats a viewer may take", () => {
    expect(joinOffered({ seats_free: 1, guest_seats_free: 0 }, false)).toBe(true);
    expect(joinOffered({ seats_free: 1, guest_seats_free: 0 }, true)).toBe(false);
    expect(joinOffered({ seats_free: 0, guest_seats_free: 0 }, false)).toBe(false);
  });

  it("turns read-only on a refused seat, follows the snapshot, and joins live once a seat is free", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { ctl, w } = await started();
    ctl.toggleComment();
    expect(ctl.state.get().commenting).toBe(true);
    w.send({ t: "refused", topic: `artifact:${ID}`, code: "no_live_seat" });
    expect(ctl.state.get().spectator).toEqual({ code: "no_live_seat", joinable: false });
    expect(ctl.state.get().commenting).toBe(false);
    // A spectator may not comment.
    ctl.toggleComment();
    expect(ctl.state.get().commenting).toBe(false);
    // The first poll: no seat free; its threads and presence are taken in.
    snap = { ...snap, threads: [thread("t1")], presence: [{ public_id: "p", display_name: "Mia", state: "here", where: null, at: "x" }] };
    await vi.waitFor(() => expect(ctl.state.get().threads.map(t => t.id)).toEqual(["t1"]));
    expect(ctl.state.get().presence).toHaveLength(1);
    expect(ctl.state.get().spectator!.joinable).toBe(false);
    // A later poll: a newer version is swapped in on a view of the latest, and a seat is free.
    snap = { ...snap, artifact: artifact(2), versions: [version(1), version(2)], seats_free: 1, built_at: 2 };
    await vi.advanceTimersByTimeAsync(SNAPSHOT_POLL_MS);
    await vi.waitFor(() => expect(ctl.state.get().spectator!.joinable).toBe(true));
    expect(ctl.state.get().data!.artifact.current_version).toBe(2);
    expect(ctl.shown()).toBe(2);
    expect(ctl.state.get().newer).toBeNull();
    // Join live asks the hub for the live topics again; they go live: seated.
    ctl.joinLive();
    expect(retries(w)).toEqual([{ t: "retry", topics: [`artifact:${ID}`, `presence:${ID}`, `working:${ID}`] }]);
    w.live();
    expect(ctl.state.get().spectator).toBeNull();
    const polled = snapshots;
    await vi.advanceTimersByTimeAsync(SNAPSHOT_POLL_MS * 2);
    // A seated view polls no more.
    expect(snapshots).toBe(polled);
    ctl.dispose();
  });

  it("takes a seat released for want of input again at the next press", async () => {
    const { ctl, w } = await started();
    w.send({ t: "refused", topic: `presence:${ID}`, code: "seat_idle" });
    expect(ctl.state.get().spectator?.code).toBe("seat_idle");
    await vi.waitFor(() => expect(snapshots).toBeGreaterThan(0));
    document.dispatchEvent(new Event("pointerdown"));
    expect(retries(w)).toHaveLength(1);
    // A second press at once asks nothing more.
    document.dispatchEvent(new Event("keydown"));
    expect(retries(w)).toHaveLength(1);
    w.live();
    expect(ctl.state.get().spectator).toBeNull();
    ctl.dispose();
  });

  it("takes a seat released for want of input again at a pointer move, or when the page shows", async () => {
    const { ctl, w } = await started();
    w.send({ t: "refused", topic: `presence:${ID}`, code: "seat_idle" });
    await vi.waitFor(() => expect(snapshots).toBeGreaterThan(0));
    document.dispatchEvent(new Event("pointermove"));
    expect(retries(w)).toHaveLength(1);
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(Date.now() + INPUT_JOIN_GAP_MS);
    document.dispatchEvent(new Event("visibilitychange"));
    expect(retries(w)).toHaveLength(2);
    vi.useRealTimers();
    // A seat refused for want of one is not asked for again by input.
    w.send({ t: "refused", topic: `presence:${ID}`, code: "no_live_seat" });
    expect(ctl.state.get().spectator?.code).toBe("no_live_seat");
    document.dispatchEvent(new Event("pointermove"));
    expect(retries(w)).toHaveLength(2);
    ctl.dispose();
  });
});
