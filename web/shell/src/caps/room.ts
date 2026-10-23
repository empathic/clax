// The `room` capability's viewer reports for a framed page. The page's
// bridge holds the room's socket on its own origin
// (web/bridge/src/caps/room-socket.ts), whose level the site fixes when it
// opens; the shell knows when its viewer changes (a guest who names
// themselves), so once the bridge asks (`watch`, a call per frame document)
// it says so (`room/viewer`), and the bridge reopens the socket under the
// same label at the new level. It sends nothing else. In the Clax app,
// whose scheme handlers carry no WebSocket, `watch` is refused
// `capability_disabled`, and the bridge then opens no socket and refuses
// every room call so. A lazy chunk (registry.ts), loaded on the page's first
// room call.
import { onViewer } from "../threads";
import { onHost } from "../transport";
import { CapError } from "./errors";
import type { HandlerFactory } from "./host";

export type Viewers = (fn: (v: unknown) => void) => () => void;

export function makeRoomHandler(viewers: Viewers = onViewer as Viewers, inApp: () => boolean = () => onHost()): HandlerFactory {
  return env => {
    let off: (() => void) | null = null;
    const stop = () => { off?.(); off = null; };
    return {
      async call(method) {
        if (method !== "watch") throw new CapError("capability_removed", `room.${String(method)} is not part of this runtime`);
        if (inApp()) throw new CapError("capability_disabled", "rooms are not available in the Clax app");
        off ??= viewers(() => env.post({ type: "clax:event", ns: "room", topic: "viewer", data: null }));
        return null;
      },
      reset: stop,
      leave: stop,
      dispose: stop,
    };
  };
}

export const roomHandler: HandlerFactory = makeRoomHandler();
