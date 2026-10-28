// The approval bar's copy and its one write: what a version asks for, what
// approving lets the page do, and the PUT that approves the declaration the
// bar showed (plan 2026-10-26-quickstart-d-permissions, Decisions 8 and 9).
import { ApiError, type PendingCapabilities, getToken } from "../api";

type Words = { label: string; detail: string };

const WORDS: Record<string, Words> = {
  net: { label: "the internet", detail: "Fetch from the hosts listed, through Clax." },
  sample: { label: "Claude", detail: "Ask Claude, paid for by this machine's Anthropic API key, within a daily spending cap." },
  account: { label: "sign-in", detail: "Sign in people to this page with their Clax account." },
  shell: { label: "Clax's controls", detail: "Point at Clax's own controls, such as Fork and Publish, to guide you through them." },
  notify: { label: "notifications", detail: "Send you notifications through the Clax app." },
  share: { label: "sharing", detail: "Open your device's share sheet from the Clax app." },
  file: { label: "files", detail: "Ask for a file on your device through the Clax app." },
  camera: { label: "the camera", detail: "Use your device's camera through the Clax app." },
  location: { label: "your location", detail: "Read your device's location through the Clax app." },
};

/** What a hosted member's approval leaves to the owner's answer on the page's first use (the owner's approval writes these grants). */
const ASKS_OWNER = new Set(["net", "sample"]);

/** How the bar, the dialog and the Permissions panel name capability `name`, as the start of a sentence: `The internet`, `Claude`, `Clax's controls`. */
export function capabilityTitle(name: string): string {
  const label = WORDS[name]?.label ?? name;
  return label[0].toUpperCase() + label.slice(1);
}

/** One thing a version asks for: the capability `name`, how the bar names it, and what approving lets the page do. */
export type PendingItem = { name: string; label: string; detail: string };

/** The items of `p` (as approved by the owner, or, with `owner` false, by a
 * member whose approval leaves `net` and `sample` to the owner's first ask), in the order the daemon lists them; `net` stands for its
 * new hosts too, and leads when only hosts are new. The label of `net` names
 * its hosts. */
export function pendingItems(p: PendingCapabilities, owner = true): PendingItem[] {
  const names = p.capabilities.includes("net") || p.hosts.length === 0 ? p.capabilities : ["net", ...p.capabilities];
  return names.map(name => {
    const w = WORDS[name] ?? { label: name, detail: `Use ${name}.` };
    const hosts = name === "net" && p.hosts.length ? ` (${p.hosts.join(", ")})` : "";
    return { name, label: w.label + hosts, detail: owner || !ASKS_OWNER.has(name) ? w.detail : `${w.detail} The owner is asked at the page's first request.` };
  });
}

/** The bar's text: `v4 asks for: the internet (api.example.com), Claude`. */
export function barText(p: PendingCapabilities): string {
  return `v${p.version} asks for: ${pendingItems(p).map(i => i.label).join(", ")}`;
}

/** `PUT /api/artifacts/<id>/capability-approval` for the declaration `p` showed.
 * Rejects with the `ApiError` of a refusal; a 409 (`stale_version`) means a
 * version or declaration came since, so the caller refetches and shows the new bar. */
export async function approveCapabilities(id: string, version: number, declarationSha256: string): Promise<void> {
  const token = await getToken();
  const res = await fetch(`/api/artifacts/${id}/capability-approval`, {
    method: "PUT",
    headers: { "content-type": "application/json", ...(token ? { authorization: `Bearer ${token}` } : {}) },
    body: JSON.stringify({ version, declaration_sha256: declarationSha256 }),
  });
  if (res.ok) return;
  let message = res.statusText;
  let code: string | null = null;
  try { const e = (await res.json()).error; message = e?.message ?? message; code = e?.code ?? null; } catch { /* not json */ }
  throw new ApiError(res.status, message, code);
}
