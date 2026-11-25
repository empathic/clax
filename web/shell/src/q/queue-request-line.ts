// A queue request's one line, apart from its routes so the inbox model
// (which every page's notifications load) carries no more than this.
import type { QueueRequest } from "../api";

/** A queue request's line, as its card reads: "<agent> asks for a report
 * queue “<name>”". */
export const queueRequestLine = (r: Pick<QueueRequest, "agent" | "name">): string => `${r.agent} asks for a report queue “${r.name}”`;
