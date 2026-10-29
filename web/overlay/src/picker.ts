// The overlay's comment mode: the bridge's own (`comment-mode.ts`), as the
// Chrome extension's overlay runs it, with each pick's anchor, where it is,
// and the DOM-rendered clip of what it points at (`clip.ts`).
import { buildElementAnchor, buildRangeAnchor } from "../../bridge/src/anchor";
import { areaBox, boxOf, buildAreaAnchor, containingElement } from "../../bridge/src/area";
import { type Rect, type RenderResult, renderAreaClipWithReport, renderTargetClipWithReport } from "../../bridge/src/clip";
import { CommentMode } from "../../bridge/src/comment-mode";
import type { Anchor } from "../../bridge/src/protocol";

/** A pick: where it points, what it is, where it shows in the viewport now
 * (CSS px), and what renders its clip from the page (a PNG, and what the
 * render left out). */
export interface Picked {
  anchor: Anchor;
  mode: "element" | "range" | "area";
  rect(): Rect;
  render(): Promise<RenderResult>;
}

/** Comment mode as the overlay drives it. */
export interface Picker {
  set(on: boolean): void;
  /** The pick's clip is taken: the area's rectangle goes, and a new pick may start. */
  done(): void;
  destroy(): void;
}

const boxRect = (b: { left: number; top: number; width: number; height: number }): Rect => ({ x: b.left, y: b.top, w: b.width, h: b.height });

/** Comment mode on `doc`: `onPick` hears each pick, `onCancel` Escape. */
export function commentPicker(doc: Document, onPick: (p: Picked) => void, onCancel: () => void): Picker {
  const win = doc.defaultView ?? window;
  const mode = new CommentMode(doc, {
    hover: () => {},
    pickElement: el => onPick({ anchor: buildElementAnchor(doc, el), mode: "element", rect: () => boxRect(el.getBoundingClientRect()), render: () => renderTargetClipWithReport(el, win) }),
    pickRange: r => {
      const range = r.cloneRange();
      onPick({ anchor: buildRangeAnchor(doc, range), mode: "range", rect: () => boxRect(range.getBoundingClientRect()), render: () => renderTargetClipWithReport(range, win) });
    },
    // As the bridge does: the clip is the drawn rectangle placed on its
    // element at release, cropped out of a render of that element.
    pickArea: r => {
      const el = containingElement(doc, r);
      const anchor = buildAreaAnchor(doc, r, undefined, el);
      const at = areaBox(anchor.area!, boxOf(el));
      onPick({ anchor, mode: "area", rect: () => areaBox(anchor.area!, boxOf(el)), render: () => renderAreaClipWithReport(el, at, win) });
    },
    cancel: onCancel,
  });
  return { set: on => mode.set(on), done: () => mode.captured(), destroy: () => mode.destroy() };
}
