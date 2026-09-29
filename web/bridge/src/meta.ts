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
