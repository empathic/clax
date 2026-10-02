// The `room` capability's shell side: one WebSocket per open document to
// `/api/artifacts/<aid>/room?peer=<label>`, plus `&token=<bearer>` when this
// shell holds the token (the daemon's frames and levels are in
// docs/contract.md "Room protocol"), in both frame modes, relayed to the page
// as `clax:event`s of ns "room". The label is kept across reconnects and
// replaced when the frame loads a new document. On reconnect the shell
// re-sends the page's latest presence and re-joins its rooms; the page
// re-sends nothing. The daemon fixes a socket's level when it opens, so when
// the viewer's name changes (a LAN viewer who names themselves moves from
// `view` to `interact`) the shell opens a new socket under the same label,
// which replaces the old one without the page seeing it leave. Only frames of
// the daemon's shapes reach the page (`checkFrame`). This module is a lazy
// chunk (registry.ts), loaded on the page's first room call.
import { onViewer } from "../threads";
import { CapError } from "./errors";
import type { HandlerFactory } from "./host";

export type SocketLike = {
  readyState: number;
  send(data: string): void;
  close(code?: number, reason?: string): void;
  onopen: ((e: unknown) => void) | null;
  onclose: ((e: { code: number; reason: string }) => void) | null;
  onmessage: ((e: { data: unknown }) => void) | null;
};
export type OpenSocket = (url: string) => SocketLike;
export type Timer = (fn: () => void, ms: number) => ReturnType<typeof setTimeout>;

/** Delays between reconnect attempts; the last repeats. */
export const RECONNECT_MS = [1000, 2000, 5000];
export const ACK_TIMEOUT_MS = 10_000;

const ALPHABET = "0123456789abcdefghijklmnopqrstuv";

/** Sixteen random characters of [0-9a-v]: one open document's peer label. */
export function peerLabel(): string {
  return [...crypto.getRandomValues(new Uint8Array(16))].map(b => ALPHABET[b & 31]).join("");
}

type Frame = { t: string; id?: number; room?: string | null; code?: string; message?: string; dropped?: boolean; [k: string]: unknown };
type Waiter = { resolve(f: Frame): void; reject(e: unknown): void; timer: ReturnType<typeof setTimeout> };

export type Viewers = (fn: (v: unknown) => void) => () => void;

// The daemon's frames are checked before any reaches the page (docs/contract.md
// "Room protocol"): a frame of another shape, or one past these bounds, is dropped.
const MAX_FRAME_CHARS = 4 * 1024 * 1024;
const MAX_PEERS = 256;
const LABEL = /^[0-9a-z]{1,64}$/;
const ROOM = /^[a-z0-9][a-z0-9_.-]{0,47}$/;
const TOPIC = /^[a-z][a-z0-9_.-]{0,47}$/;
const CODE = /^[a-z_]{1,64}$/;

const isObject = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v);
const isRoom = (v: unknown) => v === null || (typeof v === "string" && ROOM.test(v));
const isLabel = (v: unknown): v is string => typeof v === "string" && LABEL.test(v);
const isId = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v) && v > 0;

/** A viewer as the daemon sends it: `{peer, by, isMe, sameTab, kind, guest}`. */
function isWho(v: unknown): v is Record<string, unknown> {
  return isObject(v) && isLabel(v.peer) && (v.by === null || typeof v.by === "string")
    && typeof v.isMe === "boolean" && typeof v.sameTab === "boolean"
    && (v.kind === "viewer" || v.kind === "agent") && typeof v.guest === "boolean";
}
const isWirePeer = (v: unknown) => isWho(v) && isObject(v.presence);

/** The frame `raw` holds when it is one the daemon sends, else null. */
export function checkFrame(raw: unknown): Frame | null {
  if (typeof raw !== "string" || raw.length > MAX_FRAME_CHARS) return null;
  let f: unknown;
  try { f = JSON.parse(raw); } catch { return null; }
  if (!isObject(f)) return null;
  switch (f.t) {
    case "welcome": return isLabel(f.peer) ? { t: "welcome", peer: f.peer } : null;
    case "peers": return isRoom(f.room) && Array.isArray(f.peers) && f.peers.length <= MAX_PEERS && f.peers.every(isWirePeer)
      ? { t: "peers", room: f.room as string | null, peers: f.peers } : null;
    case "peer": return isRoom(f.room) && isWirePeer(f.peer) ? { t: "peer", room: f.room as string | null, peer: f.peer } : null;
    case "left": return isRoom(f.room) && isLabel(f.peer) ? { t: "left", room: f.room as string | null, peer: f.peer } : null;
    case "msg": return isRoom(f.room) && isWho(f.msg) && typeof f.msg.topic === "string" && TOPIC.test(f.msg.topic)
      ? { t: "msg", room: f.room as string | null, msg: f.msg } : null;
    case "ack": return isId(f.id) && (f.dropped === undefined || typeof f.dropped === "boolean")
      ? { t: "ack", id: f.id, ...(f.dropped === undefined ? {} : { dropped: f.dropped }) } : null;
    case "nack": return isId(f.id) && typeof f.code === "string" && CODE.test(f.code) && typeof f.message === "string"
      ? { t: "nack", id: f.id, code: f.code, message: f.message } : null;
    default: return null;
  }
}

export function makeRoomHandler(
  open: OpenSocket = url => new WebSocket(url) as unknown as SocketLike,
  timer: Timer = (fn, ms) => setTimeout(fn, ms),
  viewers: Viewers = onViewer as Viewers,
): HandlerFactory {
  return env => {
    let label = peerLabel();
    let ws: SocketLike | null = null;
    let isOpen = false;
    let wanted = false;
    let terminal = false;
    let attempt = 0;
    let seq = 0;
    let generation = 0;
    const presence = new Map<string | null, unknown>();
    const joined = new Set<string>();
    const waiting = new Map<number, Waiter>();
    // The socket a rename replaced: closed once its successor opens.
    let retired: SocketLike | null = null;

    const push = (topic: string, data: unknown) => env.post({ type: "clax:event", ns: "room", topic, data });
    // The owner shell appends the token: a WebSocket cannot send an Authorization header.
    const url = () => `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/api/artifacts/${env.aid}/room?peer=${label}`
      + (env.token ? `&token=${encodeURIComponent(env.token)}` : "");

    function failWaiters(message: string) {
      for (const w of waiting.values()) { clearTimeout(w.timer); w.reject(new CapError("upstream_error", message)); }
      waiting.clear();
    }

    function request(msg: Record<string, unknown>): Promise<Frame> {
      if (!ws || !isOpen) return Promise.reject(new CapError("upstream_error", "the room is not connected"));
      const id = ++seq;
      const s = ws;
      return new Promise((resolve, reject) => {
        const t = timer(() => { waiting.delete(id); reject(new CapError("upstream_error", "the room did not answer within 10 s")); }, ACK_TIMEOUT_MS);
        waiting.set(id, { resolve, reject, timer: t });
        s.send(JSON.stringify({ ...msg, id }));
      });
    }

    function rejoin(room: string) {
      request({ t: "join", room }).catch(e => {
        joined.delete(room);
        push("error", { room, code: e instanceof CapError ? e.code : "upstream_error", message: e instanceof Error ? e.message : String(e) });
      });
    }

    function onFrame(f: Frame) {
      switch (f.t) {
        case "welcome": case "peers": case "peer": case "left": case "msg": {
          const { t, ...data } = f;
          push(t, data);
          return;
        }
        case "ack": case "nack": {
          const w = waiting.get(f.id as number);
          if (!w) return;
          waiting.delete(f.id as number);
          clearTimeout(w.timer);
          if (f.t === "ack") w.resolve(f); else w.reject(new CapError(f.code as string, f.message as string));
          return;
        }
        default: return;
      }
    }

    function connect() {
      if (!wanted || terminal || ws) return;
      const mine = generation;
      const s = open(url());
      ws = s;
      s.onopen = () => {
        if (ws !== s) return;
        retired?.close(1000, "renamed");
        retired = null;
        isOpen = true;
        attempt = 0;
        push("connection", { connected: true });
        for (const [room, state] of presence) s.send(JSON.stringify({ t: "presence", room, state }));
        for (const room of joined) rejoin(room);
      };
      s.onmessage = e => {
        if (ws !== s) return;
        const f = checkFrame(e.data);
        if (f) onFrame(f);
      };
      s.onclose = e => {
        if (ws !== s) return;
        ws = null;
        const was = isOpen;
        isOpen = false;
        failWaiters("the room connection closed");
        if (was) push("connection", { connected: false });
        if (e.code === 4403) {
          terminal = true;
          const code = e.reason === "revoked" ? "revoked" : "not_granted";
          push("error", { room: null, code, message: code === "revoked" ? "this view's access to the room was withdrawn" : "this view cannot join the room" });
          return;
        }
        if (e.code === 4409) { terminal = true; return; }
        if (!wanted || mine !== generation) return;
        const delay = RECONNECT_MS[Math.min(attempt++, RECONNECT_MS.length - 1)];
        timer(() => { if (mine === generation) connect(); }, delay);
      };
    }

    function end() {
      generation++;
      wanted = false;
      terminal = false;
      attempt = 0;
      const s = ws;
      ws = null;
      isOpen = false;
      failWaiters("the page went away");
      s?.close(1000, "left");
      retired?.close(1000, "left");
      retired = null;
      presence.clear();
      joined.clear();
      label = peerLabel();
    }

    const offViewer = viewers(() => {
      if (!wanted || terminal || !ws) return;
      // Detached first: its close (4409 from the daemon, or ours) is not news for the page.
      const old = ws;
      old.onopen = old.onmessage = old.onclose = null;
      retired = old;
      ws = null;
      isOpen = false;
      failWaiters("the room reconnected");
      connect();
    });

    return {
      async call(method, args) {
        switch (method) {
          case "connect":
            wanted = true;
            connect();
            return null;
          case "presence": {
            const room = (args[0] ?? null) as string | null;
            presence.set(room, args[1]);
            if (ws && isOpen) ws.send(JSON.stringify({ t: "presence", room, state: args[1] }));
            return null;
          }
          case "emit": {
            if (!ws || !isOpen) return { dropped: false };
            const [room, topic] = args;
            const msg: Record<string, unknown> = { t: "emit", room: room ?? null, topic };
            if (args.length > 2) msg.data = args[2];
            const f = await request(msg);
            return { dropped: f.dropped === true };
          }
          case "join": {
            const room = String(args[0]);
            await request({ t: "join", room });
            joined.add(room);
            return null;
          }
          case "leave": {
            const room = String(args[0]);
            joined.delete(room);
            presence.delete(room);
            if (ws && isOpen) ws.send(JSON.stringify({ t: "leave", room }));
            return null;
          }
          default:
            throw new CapError("capability_removed", `room.${method} is not part of this runtime`);
        }
      },
      reset: end,
      leave: end,
      dispose() {
        offViewer();
        end();
      },
    };
  };
}

export const roomHandler: HandlerFactory = makeRoomHandler();
