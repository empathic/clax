// db.d.ts's live updates for a framed page. The page's bridge reads and
// writes its documents on its own origin (web/bridge/src/caps/origin.ts),
// whose site has no change stream; the shell hears the daemon's event
// stream, so once the bridge asks (`watch`, a call per frame document) it
// tells it what changed: `db/changed` with `{path}` for a document event,
// `{}` (every subscription) for `resync`, `version` (a republish may change
// the rules) and `ready`, and `db/stream` with `{up}` as the stream drops
// and comes back (`stream_up` or `ready`), so the bridge polls only while it
// is down. It sends nothing it was not asked for, and reads or writes nothing.
import type { ArtifactEvent } from "../events";
import { CapError } from "./errors";
import type { HandlerFactory } from "./host";

export const dbHandler: HandlerFactory = env => {
  let watching = false;
  const post = (topic: string, data: unknown) => env.post({ type: "clax:event", ns: "db", topic, data });
  return {
    async call(method) {
      if (method !== "watch") throw new CapError("capability_removed", `db.${String(method)} is not part of this runtime`);
      watching = true;
      return null;
    },
    onEvent(e: ArtifactEvent) {
      if (!watching) return;
      if (e.type === "doc") post("changed", { path: e.path });
      else if (e.type === "resync" || e.type === "version") post("changed", {});
      else if (e.type === "stream_up") post("stream", { up: true });
      else if (e.type === "stream_down") post("stream", { up: false });
      else if (e.type === "ready") {
        // A fresh stream: whatever happened before it is refetched.
        post("stream", { up: true });
        post("changed", {});
      }
    },
    reset() { watching = false; },
    dispose() { watching = false; },
  };
};
