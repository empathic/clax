// The owner's queues for the DevTools panel's Queues view (its Addendum 1):
// the worker holds the `queues` topic (a bare ping naming no queue or
// report; the gateway's live-only stream takes it) while a panel watches,
// and tells each watching panel `queue-ping` at each ping and when the
// stream comes back, after which the panel reads again. The reads, the
// reply and the resolve are the owner's own queue routes through the
// gateway; a claim is an agent's and is never asked for.
import type { HubMsg, TabMsg } from "../../../shell/src/stream-hub";
import type { WorkerToPanel } from "../messages";

export const QUEUES_HUB_ID = "queues";
type Port = Pick<chrome.runtime.Port, "postMessage" | "onDisconnect">;
export type QueuesDeps = { hub: { receive(id: string, msg: TabMsg): void; detach(id: string): void } };

export class WorkerQueues {
  private watching = new Set<Port>();
  private known = new WeakSet<Port>();

  constructor(private readonly d: QueuesDeps) {}

  /** A panel starts or stops watching; the topic is held while any does. */
  watch(port: Port, on: boolean): void {
    const was = this.watching.size;
    if (on) this.watching.add(port); else this.watching.delete(port);
    if (on && !this.known.has(port)) {
      this.known.add(port);
      port.onDisconnect.addListener(() => this.watch(port, false));
    }
    if (!was && this.watching.size) this.d.hub.receive(QUEUES_HUB_ID, { t: "topics", topics: ["queues"] });
    else if (was && !this.watching.size) this.d.hub.detach(QUEUES_HUB_ID);
  }

  /** One hub message for the client `queues`. */
  fromHub(msg: HubMsg): void {
    if (msg.t === "ping") this.d.hub.receive(QUEUES_HUB_ID, msg);
    else if (msg.t === "live" || msg.t === "resync" || (msg.t === "event" && msg.name === "queues")) this.tell();
  }

  private tell(): void {
    for (const p of this.watching) {
      try { p.postMessage({ t: "queue-ping" } satisfies WorkerToPanel); } catch { /* the panel closed */ }
    }
  }
}
