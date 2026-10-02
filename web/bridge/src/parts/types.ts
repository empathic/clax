export type CommentPart = typeof import("./comment");
export type ClipPart = typeof import("./clip");
export type CapsPart = typeof import("./caps");
export type RoomPart = typeof import("./room");
export type SamplePart = typeof import("./sample");
/** The parts' loaders. `attempt` (0 first) numbers a retry after a failure,
 * which a loader by URL loads afresh. */
export type Parts = {
  comment(attempt?: number): Promise<CommentPart>;
  clip(attempt?: number): Promise<ClipPart>;
  caps(attempt?: number): Promise<CapsPart>;
  room(attempt?: number): Promise<RoomPart>;
  sample(attempt?: number): Promise<SamplePart>;
};
