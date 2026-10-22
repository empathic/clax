// The `room` capability's connection on the page's own origin: one WebSocket
// per document to the site's `/_clax/api/room?peer=<label>` (the daemon's
// frames and levels are in docs/contract.md "Room protocol"), as whoever the
// site's cookie names, framed or top-level. Its frames reach room.ts as the
// pushes of ns "room" (`welcome`, `peers`, `peer`, `left`, `msg`,
// `connection`, `error`). The label is kept across reconnects; on reconnect
// the page's latest presence is sent again and its rooms joined again.
import { CapabilityError, type Rpc } from "../rpc";
import type { OriginClient } from "./origin-client";

/** Delays between reconnect attempts; the last repeats. */
export const RECONNECT_MS = [1000, 2000, 5000];
export const ACK_TIMEOUT_MS = 10_000;

const ALPHABET = "0123456789abcdefghijklmnopqrstuv";

/** Sixteen random characters of [0-9a-v]: one open document's peer label. */
export function peerLabel(): string {
  return [...crypto.getRandomValues(new Uint8Array(16))].map(b => ALPHABET[b & 31]).join("");
}

type Frame = { t: string; id?: number; code?: string; message?: string; dropped?: boolean; [k: string]: unknown };
type Waiter = { resolve(f: Frame): void; reject(e: unknown): void; timer: ReturnType<typeof setTimeout> };

/** The room's connection for room.ts's `roomNamespace`. */
export function originRoom(client: Pick<OriginClient, "socket">): Pick<Rpc, "call" | "on"> {
  let label = peerLabel();
  const listeners = new Map<string, Set<(d: unknown) => void>>();
  const push = (topic: string, data: unknown) => { for (const fn of [...(listeners.get(topic) ?? [])]) fn(data); };
  let ws: WebSocket | null = null;
  let isOpen = false;
  let wanted = false;
  let terminal = false;
  let attempt = 0;
  let seq = 0;
  const presence = new Map<string | null, unknown>();
  const joined = new Set<string>();
  const waiting = new Map<number, Waiter>();

  const send = (m: unknown) => { if (ws && isOpen) ws.send(JSON.stringify(m)); };

  function failWaiters(message: string) {
    for (const w of waiting.values()) { clearTimeout(w.timer); w.reject(new CapabilityError("upstream_error", message)); }
    waiting.clear();
  }

  function request(msg: Record<string, unknown>): Promise<Frame> {
    if (!ws || !isOpen) return Promise.reject(new CapabilityError("upstream_error", "the room is not connected"));
    const id = ++seq;
    const s = ws;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { waiting.delete(id); reject(new CapabilityError("upstream_error", "the room did not answer within 10 s")); }, ACK_TIMEOUT_MS);
      waiting.set(id, { resolve, reject, timer });
      s.send(JSON.stringify({ ...msg, id }));
    });
  }

  function rejoin(room: string) {
    request({ t: "join", room }).catch(e => {
      joined.delete(room);
      push("error", { room, code: e instanceof CapabilityError ? e.code : "upstream_error", message: e instanceof Error ? e.message : String(e) });
    });
  }

  function onFrame(f: Frame) {
    if (f.t === "ack" || f.t === "nack") {
      const w = waiting.get(f.id as number);
      if (!w) return;
      waiting.delete(f.id as number);
      clearTimeout(w.timer);
      // A spectator's emit (spec 2026-10-11-clax-dev-hosted-design §16.1)
      // is the contract's transient `upstream_error`.
      if (f.t === "ack") w.resolve(f);
      else w.reject(new CapabilityError(f.code === "spectator" ? "upstream_error" : String(f.code), String(f.message)));
      return;
    }
    const { t, ...data } = f;
    if (["welcome", "peers", "peer", "left", "msg"].includes(t)) push(t, data);
  }

  function connect() {
    if (!wanted || terminal || ws) return;
    let s: WebSocket;
    try {
      s = client.socket(`room?peer=${label}`);
    } catch {
      terminal = true;
      push("error", { room: null, code: "not_granted", message: "this view cannot join the room" });
      return;
    }
    ws = s;
    s.onopen = () => {
      if (ws !== s) return;
      isOpen = true;
      attempt = 0;
      push("connection", { connected: true });
      for (const [room, state] of presence) send({ t: "presence", room, state });
      for (const room of joined) rejoin(room);
    };
    s.onmessage = e => {
      if (ws !== s || typeof e.data !== "string") return;
      let f: unknown;
      try { f = JSON.parse(e.data); } catch { return; }
      if (f && typeof f === "object" && typeof (f as Frame).t === "string") onFrame(f as Frame);
    };
    s.onclose = e => {
      if (ws !== s) return;
      ws = null;
      const was = isOpen;
      isOpen = false;
      failWaiters("the room connection closed");
      if (was) push("connection", { connected: false });
      // Another viewer's socket holds the label: take a fresh one.
      if (e.code === 4403 && e.reason === "peer_taken") label = peerLabel();
      else if (e.code === 4403 || e.code === 4409) {
        terminal = true;
        if (e.code === 4403) {
          const code = e.reason === "revoked" ? "revoked" : "not_granted";
          push("error", { room: null, code, message: code === "revoked" ? "this view's access to the room was withdrawn" : "this view cannot join the room" });
        }
        return;
      }
      setTimeout(connect, RECONNECT_MS[Math.min(attempt++, RECONNECT_MS.length - 1)]);
    };
  }

  // A page leaving for good (not kept whole in the back/forward cache) closes its socket.
  addEventListener("pagehide", e => {
    if (e.persisted) return;
    wanted = false;
    ws?.close(1000, "left");
  });

  return {
    async call(_ns, method, args) {
      switch (method) {
        case "connect":
          wanted = true;
          connect();
          return null;
        case "presence": {
          const room = (args[0] ?? null) as string | null;
          presence.set(room, args[1]);
          send({ t: "presence", room, state: args[1] });
          return null;
        }
        case "emit": {
          if (!ws || !isOpen) return { dropped: false };
          const msg: Record<string, unknown> = { t: "emit", room: args[0] ?? null, topic: args[1] };
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
          send({ t: "leave", room });
          return null;
        }
        default:
          throw new CapabilityError("capability_removed", `room.${method} is not part of this runtime`);
      }
    },
    on(_ns, topic, fn) {
      let set = listeners.get(topic);
      if (!set) listeners.set(topic, (set = new Set()));
      set.add(fn);
      return () => { set!.delete(fn); };
    },
  };
}
