// Who this browser is and whether it may start an artifact (`GET
// /api/viewers/me`), asked once per page for the New artifact tile, which
// the gallery moves between its sections. A module of the tile's alone,
// so it stays in the gallery's lazy chunk.
import type { Me } from "../view/new-artifact";

type Asked = { viewer?: Me; may_create?: boolean } | null;
let asked: Promise<Asked> | null = null;

export function mayCreate(): Promise<Asked> {
  asked ??= fetch("/api/viewers/me").then(r => r.json() as Promise<Asked>, () => null);
  return asked;
}

/** Forgets the answer (tests that change it). */
export function forgetViewer(): void { asked = null; }
