// The annotate plugin: the reporter draws on the screenshot with Clax's
// Annotator (Box, Arrow, Highlight, Redact, Text; Undo and Clear), the
// marks drawn into the PNG that is sent (annotate.ts).
import { type AnnotateTool, type ClaxPlugin, definePlugin, PLUGIN_API_VERSION } from "../../../sdk/src/plugin";
import Annotator from "../Annotator.svelte";

/** Every tool, in the order the Annotator shows them. */
export const ANNOTATE_TOOLS: AnnotateTool[] = [
  { id: "box", label: "Box" },
  { id: "arrow", label: "Arrow" },
  { id: "highlight", label: "Highlight" },
  { id: "redact", label: "Redact" },
  { id: "text", label: "Text" },
];

/** Drawing on the screenshot with `tools` (ids; default every tool). */
export const annotate = (opts: { tools?: string[] } = {}): ClaxPlugin => definePlugin({
  name: "annotate", version: "1.0.0", apiVersion: PLUGIN_API_VERSION, capabilities: [],
  contributes: { annotate: { tools: opts.tools ? ANNOTATE_TOOLS.filter(t => opts.tools!.includes(t.id)) : ANNOTATE_TOOLS, ui: Annotator as unknown } },
});
