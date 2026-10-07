// The shell's sidebar groups threads by page (`anchor.file`). On a live page
// the route plays that part (spec 2026-10-05 §7): threads on other routes
// read "on ?tab=billing", and the route-less view is the index. A thread's
// clip is served only with the credential, which the panel does not hold:
// the worker fetches it for the panel (`PanelLink.clip`).
import { INDEX_FILE } from "../../../bridge/src/protocol";
import type { Thread } from "../../../shell/src/threads";

export const pageOfRoute = (route: string | null): string => route ?? INDEX_FILE;
export const asPages = (threads: Thread[]): Thread[] =>
  threads.map(t => ({ ...t, anchor: { ...t.anchor, file: pageOfRoute(t.anchor.route ?? null) } }));
