// The page's identity, read from the bridge's own script tag, and the hello
// that announces it to the shell.

import { type BridgeToShell, INDEX_FILE } from "./protocol";

/** The artifact, version, contract, and file (published path) of this page. */
export interface PageMeta { artifact: string; version: number; contract: string; file: string }

/** The page's identity from the bridge tag's `data-*` attributes; a tag
 * without `data-file` is the index's. */
export function readMeta(script: HTMLScriptElement | null): PageMeta {
  return {
    artifact: script?.dataset.artifact ?? "",
    version: Number(script?.dataset.version ?? "0"),
    contract: script?.dataset.contract ?? "",
    file: script?.dataset.file || INDEX_FILE,
  };
}

/** The message reporting the page's fragment `hash` to the shell. */
export function hashFor(hash: string): Extract<BridgeToShell, { type: "artifax:hash" }> {
  return { type: "artifax:hash", hash };
}

/** The `artifax:hello` this page greets the shell with. */
export function helloFor(meta: PageMeta): Extract<BridgeToShell, { type: "artifax:hello" }> {
  return { type: "artifax:hello", artifact: meta.artifact, version: meta.version, file: meta.file };
}

/** Whether `src` is a bridge URL exactly as the daemon writes it: the bare
 * `/_artifax/bridge.js` or `/_artifax/bridge.js?v=<lowercase hex>`. */
export function isBridgeSrc(src: string | null): boolean {
  return src !== null && /^\/_artifax\/bridge\.js(\?v=[0-9a-f]+)?$/.test(src);
}

/** Whether `script` is the first daemon bridge tag (bridge URL and
 * `data-artifact`) in its document. Another copy of the bridge in the same
 * document stands down; a script that cannot be identified (no
 * `currentScript`) is taken as the first. */
export function isFirstBridge(script: HTMLScriptElement | null): boolean {
  if (!script) return true;
  const first = Array.from(script.ownerDocument.querySelectorAll<HTMLScriptElement>("script[src][data-artifact]"))
    .find(s => isBridgeSrc(s.getAttribute("src")));
  return first === script;
}
