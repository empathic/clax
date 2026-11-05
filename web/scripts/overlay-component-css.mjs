// The overlay's components' CSS, moved into its script: shared by the
// overlay's classic script (vite.overlay.config.ts) and the SDK package's
// `./overlay` entry (build-sdk.mjs).

/** The string literal in overlay/src/mount.ts that the components' CSS replaces. */
export const CSS_SLOT = "__CLAX_OVERLAY_COMPONENT_CSS__";

/** A Vite plugin that moves the CSS the build extracts from the Svelte
 * components into the chunk `fileName` (or, given several names, the one
 * of them each output holds: a build of several formats writes one per
 * output), as the string `CSS_SLOT` stood for, and emits no .css file: the
 * overlay adopts it as a constructed sheet in its shadow root, which a
 * page's CSP `style-src` without 'unsafe-inline' allows, where the <style>
 * elements Svelte's injected CSS makes are refused. The build fails when
 * the chunk has no slot or the components compiled to no CSS.
 *
 * With several outputs, one renders at a time. `vite build` writes them in
 * turn, but Rollup's watcher starts them all together, and vite:css-post
 * keeps "the single CSS file is emitted" in one flag, so only the first output
 * would get the sheet and `--watch` would fail on the other, every rebuild.
 * Waiting in a sequential renderStart, before Vite's resets that flag, gives
 * the watcher what a plain build does. */
export function componentCssIntoScript(fileName) {
  const names = Array.isArray(fileName) ? fileName : [fileName];
  let turn = Promise.resolve();
  let done = () => {};
  return {
    name: "clax-overlay-component-css",
    apply: "build",
    enforce: "post",
    buildStart() {
      turn = Promise.resolve();
    },
    renderStart: {
      order: "pre",
      sequential: true,
      async handler() {
        const previous = turn;
        let release;
        turn = new Promise(resolve => { release = resolve; });
        await previous;
        done = release;
      },
    },
    renderError() {
      done();
    },
    generateBundle(_, bundle) {
      const sheets = Object.values(bundle).filter(f => f.type === "asset" && f.fileName.endsWith(".css"));
      const css = sheets.map(f => String(f.source)).join("\n");
      for (const f of sheets) delete bundle[f.fileName];
      const slot = new RegExp(`(["'\`])${CSS_SLOT}\\1`);
      // The named chunk, else (a build that splits the mount into a chunk the
      // overlay and the loader share) the one chunk that holds the slot.
      const named = names.find(n => n in bundle);
      const name = named && slot.test(bundle[named].code ?? "") ? named : Object.keys(bundle).find(k => bundle[k].type === "chunk" && slot.test(bundle[k].code)) ?? named ?? names.join(" or ");
      const js = bundle[name];
      if (js?.type !== "chunk" || !slot.test(js.code)) return this.error(`${name} has no ${CSS_SLOT} for the components' CSS`);
      if (!css) return this.error("the overlay's components compiled to no CSS");
      js.code = js.code.replace(slot, () => JSON.stringify(css));
      done();
    },
  };
}
