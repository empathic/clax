// A browser of this machine that holds no credential of the local daemon's
// (no owner browser session, no viewer): the daemon answers its requests 401
// `no_credential`, and serves a page load the same page this shows. Opening
// Clax from the terminal (`clax open`, or `clax login` for a link) signs the
// browser in once.

/** The heading the page shows. */
export const COLD_HEADING = "Open Clax from your terminal";

/** Whether `r` is the local daemon's refusal of a request that carries no
 * credential (401 `no_credential`). Reads a copy of its body. */
export async function isColdRefusal(r: Response): Promise<boolean> {
  if (r.status !== 401) return false;
  try {
    const b = (await r.clone().json()) as { error?: { code?: unknown } } | null;
    return b?.error?.code === "no_credential";
  } catch {
    return false;
  }
}

/** Replaces the page with how to open Clax from the terminal, once. */
export function showColdBrowser(doc: Document = document): void {
  if (doc.getElementById("clax-cold")) return;
  const main = doc.createElement("main");
  main.id = "clax-cold";
  main.style.cssText = "max-width:32rem;margin:20vh auto 0;padding:0 16px;font:16px/1.5 system-ui,sans-serif";
  const h = doc.createElement("h1");
  h.textContent = COLD_HEADING;
  h.style.fontSize = "1.25rem";
  const p = doc.createElement("p");
  const code = (t: string) => { const c = doc.createElement("code"); c.textContent = t; return c; };
  p.append("Run ", code("clax open"), " (or ", code("clax login"), " for a link).");
  const why = doc.createElement("p");
  why.textContent = "This browser is not signed in to this machine's Clax yet. Opening it from the terminal signs it in once.";
  main.append(h, p, why);
  doc.body.replaceChildren(main);
  doc.title = "Open Clax";
}
