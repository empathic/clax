// What the artifact view's sidebar island (SidebarIsland.svelte) asks of
// the thread list's chunk once it is in, so none of it is in the artifact
// entry: whether the list may show yet (the owner or a connected person,
// with a thread open and sent to an agent, waits for their approval
// requests, so each card above its thread is drawn with the list), and
// what the next load's skeleton and strip are drawn from
// (view/drawer-shape.ts).
import type { ViewState } from "../view/artifact-controller";
import { keepDrawer, measureDrawer } from "../view/drawer-shape";
import { approvalsDrawn } from "./thread-approvals";

/** Settles once artifact `id`'s thread list may show, as the view `s`
 * stands: at once, or once this viewer's approval requests are in
 * (`approvalsDrawn`, capped). */
export function listReady(id: string, s: Pick<ViewState, "me" | "threads">): Promise<void> {
  const approves = !!(s.me?.owner || s.me?.person) && s.threads.some(t => t.status === "open" && t.sent_to_agent);
  return approves ? approvalsDrawn(id) : Promise.resolve();
}

/** Keeps, for artifact `id`, whether it has a thread (`threads`, how many),
 * and with the list shown in `drawer`, the list's blocks; the skeleton
 * then goes. */
export function keepShape(id: string, threads: number, drawer: Element | null): void {
  keepDrawer(id, { t: threads ? 1 : 0, ...(drawer ? measureDrawer(drawer, innerHeight) : {}) });
  if (drawer) document.querySelector(".viewer > .dsk")?.remove();
}
