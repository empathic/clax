export type CommentPart = typeof import("./comment");
export type ClipPart = typeof import("./clip");
export type CapsPart = typeof import("./caps");
export type Parts = { comment(): Promise<CommentPart>; clip(): Promise<ClipPart>; caps(): Promise<CapsPart> };
