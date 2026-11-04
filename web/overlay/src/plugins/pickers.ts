// Picker plugins: how a report points at the page. Element (pointer, a text
// selection, or the keyboard: E again, the arrow keys and Enter), Area (a
// drag) and "this page" (the visible page at once) are the overlay's comment
// mode; the code-line picker refines a pick that lands in code with its
// file and lines (codeContext in prod Changeling), sent as `context.app.code`.
import { definePlugin, PLUGIN_API_VERSION, type PickerContribution } from "../../../sdk/src/plugin";

export const elementPicker = () => definePlugin({
  name: "picker-element", version: "1.0.0", apiVersion: PLUGIN_API_VERSION, capabilities: ["dom:read"],
  contributes: { pickers: [{ id: "element", label: "Element", key: "E", mode: "element" }] },
});

export const areaPicker = () => definePlugin({
  name: "picker-area", version: "1.0.0", apiVersion: PLUGIN_API_VERSION, capabilities: ["dom:read"],
  contributes: { pickers: [{ id: "area", label: "Area", key: "A", mode: "area" }] },
});

/** "This page": the visible page, at once. */
export const pagePicker = (opts: { label?: string } = {}) => definePlugin({
  name: "picker-page", version: "1.0.0", apiVersion: PLUGIN_API_VERSION, capabilities: ["dom:read"],
  contributes: { pickers: [{ id: "page", label: opts.label ?? "Screen", key: "S", mode: "screen" }] },
});

/** Where a pick in code is: the file, its lines, and which side of a diff and at which commit, when known. */
export interface CodeLocation {
  file: string;
  start: number;
  end: number;
  side?: "LEFT" | "RIGHT";
  sha?: string;
}

/** The code cell `node` lies in: the nearest element with `data-clax-file`
 * and `data-clax-line` (and, if it has them, `data-clax-side`, `data-clax-sha`). */
function cellOf(node: Node | null): { file: string; line: number; side?: "LEFT" | "RIGHT"; sha?: string } | null {
  const el = node instanceof Element ? node : node?.parentElement ?? null;
  const cell = el?.closest<HTMLElement>("[data-clax-file][data-clax-line]");
  if (!cell) return null;
  const line = Number(cell.dataset.claxLine);
  const file = cell.dataset.claxFile ?? "";
  if (!file || !Number.isSafeInteger(line) || line < 1) return null;
  const side = cell.dataset.claxSide === "LEFT" || cell.dataset.claxSide === "RIGHT" ? cell.dataset.claxSide : undefined;
  const sha = cell.dataset.claxSha && /^[0-9a-f]{7,64}$/i.test(cell.dataset.claxSha) ? cell.dataset.claxSha : undefined;
  return { file, line, ...(side ? { side } : {}), ...(sha ? { sha } : {}) };
}

/** The default locator: a pick on a code line (`data-clax-file` +
 * `data-clax-line`), or a text range from one line to another of the same
 * file and side. */
export function locateCode(target: Element | Range | null): CodeLocation | null {
  if (!target) return null;
  const a = cellOf(target instanceof Range ? target.startContainer : target);
  const b = target instanceof Range ? cellOf(target.endContainer) : a;
  // Both ends in code, in one file and on one side: else it is not a code pick.
  if (!a || !b || a.file !== b.file || a.side !== b.side) return null;
  return { file: a.file, start: Math.min(a.line, b.line), end: Math.max(a.line, b.line), ...(a.side ? { side: a.side } : {}), ...(a.sha ? { sha: a.sha } : {}) };
}

/** A pick in code records where it is. `locate` (default `locateCode`) is the site's: a diff view knows its files, lines and commits. */
export const codeLine = (opts: { locate?: (target: Element | Range | null, doc: Document) => CodeLocation | null } = {}) => {
  const locate = opts.locate ?? locateCode;
  const picker: PickerContribution = {
    id: "code-line", label: "Code line",
    refine(target, doc) {
      const at = locate(target, doc);
      return at ? { code: at } : null;
    },
  };
  return definePlugin({ name: "picker-code-line", version: "1.0.0", apiVersion: PLUGIN_API_VERSION, capabilities: ["dom:read"], contributes: { pickers: [picker] } });
};
