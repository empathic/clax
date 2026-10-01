// Comment mode, anchoring and areas: loaded once the page has parsed, on the
// first shell order that needs them (in practice right after the welcome).
export { AnchorCache, buildElementAnchor, buildRangeAnchor, cssPath, resolveAnchor } from "../anchor";
export type { Resolved } from "../anchor";
export { areaBox, boxOf, buildAreaAnchor, containingElement, placeArea } from "../area";
export { blockAncestor } from "../block";
export { CommentMode } from "../comment-mode";
