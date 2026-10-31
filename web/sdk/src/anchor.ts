// The SDK's anchors entry (`@empathic/clax-sdk/anchor`, scripts/build-sdk.mjs):
// how a comment holds on to what it is about on a page. An anchor is built
// from an element, a text range or a dragged area, and resolved again on a
// later load of the page, by its exact element, its selector or its quote.
// Comment mode is the pointer interaction that picks one. Everything here is
// web/bridge's, which the overlay and the Chrome extension share.
export {
  type Anchor, type AnchorArea, type AnchorResult, type Box, type ResolveMethod, INDEX_FILE, MAX_NAME,
} from "../../bridge/src/protocol";
export { buildElementAnchor, buildRangeAnchor, OVERLAY_TAG, type Resolved, resolveAnchor, textIndex, type TextIndex } from "../../bridge/src/anchor";
export { type AreaRect, buildAreaAnchor, placeArea } from "../../bridge/src/area";
export { rectOf } from "../../bridge/src/target";
export { CommentMode, type ModeHooks } from "../../bridge/src/comment-mode";
