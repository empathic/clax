// The overlay's components' CSS, moved into its script: shared by the
// overlay's classic script (vite.overlay.config.ts) and the SDK package's
// `./overlay` entry (build-sdk.mjs).

/** The string literal in overlay/src/main.ts that the components' CSS replaces. */
export const CSS_SLOT = "__CLAX_OVERLAY_COMPONENT_CSS__";

/** A Vite plugin that moves the CSS the build extracts from the Svelte
 * components into the chunk `fileName`, as the string `CSS_SLOT` stood
 * for, and emits no .css file: the overlay adopts it as a constructed sheet
 * in its shadow root, which a page's CSP `style-src` without
 * 'unsafe-inline' allows, where the <style> elements Svelte's injected CSS
 * makes are refused. The build fails when the chunk has no slot or the
 * components compiled to no CSS. */
export function componentCssIntoScript(fileName) {
  return {
    name: "clax-overlay-component-css",
    apply: "build",
    enforce: "post",
    generateBundle(_, bundle) {
      const sheets = Object.values(bundle).filter(f => f.type === "asset" && f.fileName.endsWith(".css"));
      const css = sheets.map(f => String(f.source)).join("\n");
      for (const f of sheets) delete bundle[f.fileName];
      const js = bundle[fileName];
      const slot = new RegExp(`(["'\`])${CSS_SLOT}\\1`);
      if (js?.type !== "chunk" || !slot.test(js.code)) return this.error(`${fileName} has no ${CSS_SLOT} for the components' CSS`);
      if (!css) return this.error("the overlay's components compiled to no CSS");
      js.code = js.code.replace(slot, () => JSON.stringify(css));
    },
  };
}
