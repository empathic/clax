// The shared worker that holds the browser's one `/api/stream` connection
// for every Clax tab of this origin (see `stream-hub.ts`). Each tab connects
// a port, says `hello` with the name of a Web Lock it holds while it lives,
// and sends its topics; a tab that ends without `bye` (a crash, a killed
// process) is dropped when its lock frees, or when its port closes.
import { Hub, type TabMsg } from "./stream-hub";

// The worker's global, typed by what this file uses (the DOM library has no
// SharedWorkerGlobalScope).
const scope = self as unknown as { onconnect: ((e: MessageEvent) => void) | null; navigator?: { locks?: LockManager } };

const ports = new Map<string, MessagePort>();
const locks = scope.navigator?.locks;
const hub = new Hub({
  notify: true,
  send(ids, msg) { for (const id of ids) ports.get(id)?.postMessage(msg); },
  watchLock: locks ? (name, gone) => { void locks.request(name, () => gone()).catch(() => {}); } : undefined,
});
let next = 0;

scope.onconnect = (e: MessageEvent) => {
  const port = e.ports[0];
  const id = `p${++next}`;
  ports.set(id, port);
  const gone = () => { if (ports.get(id) === port) { ports.delete(id); hub.detach(id); } };
  port.onmessage = (m: MessageEvent<TabMsg>) => {
    if (!ports.has(id)) ports.set(id, port);
    hub.receive(id, m.data);
    if (m.data?.t === "bye") { ports.delete(id); port.close(); }
  };
  // Where the browser says so, a port whose tab went away closes.
  port.addEventListener("close", gone);
  port.start();
};
