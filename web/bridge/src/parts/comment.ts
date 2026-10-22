// Comment mode, anchoring, areas and the live layers' hand-off (the live
// part loads on need): loaded once the page has parsed, on the first shell
// order that needs them (in practice right after the welcome).
export { AnchorCache, buildElementAnchor, buildRangeAnchor, cssPath, resolveAnchor } from "../anchor";
export type { Resolved } from "../anchor";
export { areaBox, boxOf, buildAreaAnchor, containingElement, placeArea } from "../area";
export { blockAncestor } from "../block";
export { describeElement as desc } from "../describe";
export { CommentMode } from "../comment-mode";
export { LiveHook } from "../live-hook";
