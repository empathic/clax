import { describe, expect, it } from "vitest";
import { LIVE_CURSOR_MS, type ShellToBridge } from "../../../bridge/src/protocol";
import type { ViewState } from "./artifact-controller";
import { RECONNECT_MS, type SocketLike } from "./live-layers";
import { LiveView, emptyLive } from "./live-view";

const T = "01J9Z3K4M5N6P7Q8R9S0T1V2W3";
const point = { path: [0, 1], offset: 4, x: 0, y: 0 };
const advance = (ms: number) => (globalThis as unknown as { claxTestClock: { advance(ms: number): void } }).claxTestClock.advance(ms);

function setup() {
  let state = { file: "index.html", pinnedVersion: null, data: { artifact: { current_version: 2, participants: { people: [{ public_id: "u_mia", display_name: "Mia", seen: null }], agents: [] } } }, draft: null, selected: null, presence: [], live: emptyLive() } as unknown as ViewState;
  const framed: ShellToBridge[] = [];
  const sent: unknown[] = [];
  let sock: SocketLike | null = null;
  const view = new LiveView({
    id: "7q3k9mzx2b4t",
    shown: s => s.pinnedVersion ?? s.data?.artifact.current_version ?? 0,
    set: f => { const prev = state; state = { ...state, ...f(state) }; view.react(prev, state); },
  }, () => state, m => framed.push(m), () => {
    sock = { readyState: 0, onopen: null, onmessage: null, onclose: null, send: d => sent.push(JSON.parse(d)), close() {} };
    return sock;
  });
  view.start();
  sock!.onopen!({});
  const change = (patch: Partial<ViewState>) => { const prev = state; state = { ...state, ...patch }; view.react(prev, state); };
  return { view, framed, sent, sock: () => sock!, state: () => state, change };
}

describe("LiveView", () => {
  it("on a 4429 drops the others' layers and opens the socket again, the view still live: the stream alone makes a spectator", () => {
    const { view, sock, state } = setup();
    const first = sock();
    first.onmessage!({ data: JSON.stringify({ t: "live", senders: [{ person_id: "u_mia", color: 2, cursor: { version: 2, file: "index.html", anchor: point, selection: null } }], drop: [], others: 0, present: 1 }) });
    expect(Object.keys(state().live!.layer.peers)).toEqual(["u_mia"]);
    // A seat frees between the socket's request and the stream's: the
    // socket is refused, the stream seated.
    first.onclose!({ code: 4429, reason: "no_live_seat" });
    expect(state().live!.layer.peers).toEqual({});
    expect((state() as { spectator?: unknown }).spectator).toBeUndefined();
    advance(RECONNECT_MS[0]);
    const second = sock();
    expect(second).not.toBe(first);
    second.onopen!({});
    // The stream refused: the controller pauses the view; Join live resumes it.
    view.pause();
    advance(60_000);
    expect(sock()).toBe(second);
    view.resume();
    expect(sock()).not.toBe(second);
    view.dispose();
  });

  it("sends the frame's pointer and selection on the shown file and version, the open pick's selection in its place", () => {
    const { view, sent, change } = setup();
    view.frameCursor(point, null);
    advance(LIVE_CURSOR_MS);
    expect(sent.filter(m => (m as { t: string }).t === "cursor").at(-1)).toEqual({ t: "cursor", version: 2, file: "index.html", anchor: point, selection: null });
    // The frame's last pick goes in place of its selection while the pick's composer is open.
    const live = { kind: "element" as const, path: [0, 2] };
    view.frameCursor(point, null, live);
    advance(LIVE_CURSOR_MS);
    expect(sent.at(-1)).toMatchObject({ t: "cursor", selection: null });
    change({ draft: { pickId: "p", anchor: {} as never, version: 2, clip: null } });
    advance(LIVE_CURSOR_MS);
    expect(sent.at(-1)).toMatchObject({ t: "cursor", selection: live });
    // A composer the page opened is not the viewer's pick.
    change({ draft: { pickId: "q", anchor: {} as never, version: 2, clip: null, byPage: true } });
    advance(LIVE_CURSOR_MS);
    expect(sent.at(-1)).toMatchObject({ t: "cursor", selection: null });
    change({ draft: null });
    advance(LIVE_CURSOR_MS);
    expect(sent.at(-1)).toMatchObject({ t: "cursor", selection: null });
    change({ selected: T });
    expect(sent.at(-1)).toEqual({ t: "open", thread_id: T });
    view.dispose();
  });

  it("draws others in the frame by opaque key and colour, and names their tags itself", () => {
    const { view, framed, sock, state } = setup();
    view.hello();
    sock().onmessage!({ data: JSON.stringify({ t: "live", others: 0, drop: [], senders: [{ person_id: "u_mia", color: 1, cursor: { version: 2, file: "index.html", anchor: point, selection: null } }] }) });
    const peers = framed.filter(m => m.type === "clax:peers").at(-1) as Extract<ShellToBridge, { type: "clax:peers" }>;
    expect(peers.peers).toEqual([{ key: "k1", color: "#8a3fd1", anchor: point, selection: null }]);
    expect(JSON.stringify(framed)).not.toContain("u_mia");
    expect(JSON.stringify(framed)).not.toContain("Mia");
    view.peerAt([{ key: "k1", x: 100, y: 40 }]);
    expect(state().live!.tags).toEqual([{ key: "k1", name: "Mia", color: 1, x: 100, y: 40 }]);
    // The socket closing takes everyone's state with it.
    sock().onclose!({ code: 4410 });
    expect(state().live!.layer.peers).toEqual({});
    expect(state().live!.tags).toEqual([]);
    view.dispose();
  });

  it("sends the cursor at most every LIVE_CURSOR_MS, the latest always, however often the frame reports", () => {
    const { view, sent } = setup();
    const cursors = () => sent.filter(m => (m as { t: string }).t === "cursor");
    // The page's own scripts can post clax:cursor as often as they like.
    for (let i = 0; i < 50; i++) view.frameCursor({ ...point, offset: i }, null);
    expect(cursors()).toHaveLength(1);
    advance(LIVE_CURSOR_MS);
    expect(cursors()).toHaveLength(2);
    expect(cursors().at(-1)).toMatchObject({ anchor: { offset: 49 } });
    // Nothing new: nothing more goes.
    advance(LIVE_CURSOR_MS * 3);
    expect(cursors()).toHaveLength(2);
    view.dispose();
  });

  it("tells the frame someone else is here, with no one to draw, so it reports the pointer", () => {
    const { view, framed, sock } = setup();
    view.hello();
    const last = () => framed.filter(m => m.type === "clax:peers").at(-1) as Extract<ShellToBridge, { type: "clax:peers" }>;
    expect(last()).toEqual({ type: "clax:peers", peers: [], here: false });
    sock().onmessage!({ data: JSON.stringify({ t: "live", others: 0, drop: [], senders: [], present: 1 }) });
    expect(last()).toEqual({ type: "clax:peers", peers: [], here: true });
    expect(JSON.stringify(framed)).not.toMatch(/u_|present/);
    view.dispose();
  });

  it("sends drafts again when the socket reopens", () => {
    const { view, sent, sock } = setup();
    view.text(null, "Label the axes");
    expect(sent.at(-1)).toEqual({ t: "typing", thread_id: null });
    sock().onopen!({});
    expect(sent).toContainEqual({ t: "draft", thread_id: null, text: "Label the axes" });
    view.dispose();
  });
});
