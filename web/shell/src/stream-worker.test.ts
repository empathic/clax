import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MessagePort as NodePort } from "node:worker_threads";
import { MOUNT_TIMEOUT_MS, WAIT_MS } from "./test/timeouts";

const A = "7q3k9mzx2b4t";

type Scope = { onconnect: ((e: { ports: unknown[] }) => void) | null };

/** The page's end of its port to the worker; `worker` is the worker's end,
 * which a test hands to each worker instance it starts. */
let worker: NodePort | null = null;
class ChannelWorker {
  readonly port: NodePort;
  constructor() {
    const { port1, port2 } = new MessageChannel() as unknown as { port1: NodePort; port2: NodePort };
    this.port = port1;
    worker = port2;
  }
  addEventListener() {}
}

/** Starts a fresh instance of the worker's script and hands it the port, as
 * a browser does on `connect`: once for a new worker, and again for a port
 * still open when the browser relaunches the worker. */
async function startWorker(): Promise<void> {
  vi.resetModules();
  (self as unknown as Scope).onconnect = null;
  await import("./stream-worker");
  (self as unknown as Scope).onconnect?.({ ports: [worker] });
}

let streams: string[];
let page: { close(): void } | null = null;
beforeEach(() => {
  worker = null;
  streams = [];
  vi.stubGlobal("SharedWorker", ChannelWorker);
  vi.stubGlobal("fetch", vi.fn(async (u: string) => {
    if (String(u).endsWith("/api/stream")) {
      streams.push(String(u));
      return new Promise<Response>(() => {});
    }
    return new Response(JSON.stringify({ token: "t" }));
  }));
});
afterEach(() => {
  page?.close();
  page = null;
  worker?.close();
  (self as unknown as Scope).onconnect = null;
  vi.unstubAllGlobals();
});

const until = { timeout: WAIT_MS };

describe("the shared worker", { timeout: MOUNT_TIMEOUT_MS }, () => {
  it("opens the stream again for a tab whose port it takes after the browser relaunched it", async () => {
    const { pageStream } = await import("./stream");
    const p = pageStream();
    page = p;
    p.watch([`artifact:${A}`], () => {});
    await vi.waitFor(() => expect(worker).not.toBeNull(), until);
    await startWorker();
    await vi.waitFor(() => expect(streams).toHaveLength(1), until);
    // WebKit ends a shared worker and starts it again with the tab's port
    // still open, and the tab is not told: the new instance knows nothing
    // of the tab until the tab speaks.
    await startWorker();
    await vi.waitFor(() => expect(streams).toHaveLength(2), until);
  });
});
