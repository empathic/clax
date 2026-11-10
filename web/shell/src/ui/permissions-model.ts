// The Permissions panel's rows: what a page may use, one row each, with how
// to take it back. Built from `GET /api/artifacts/<id>/permissions`.
import { capabilityTitle, pendingItems } from "../caps/approval";
import type { PermissionState } from "../caps/grants";
import type { PendingCapabilities } from "../api";

/** The daemon's answer to `GET /api/artifacts/<id>/permissions`. */
export type PermissionsView = {
  id: string;
  version: number;
  declared: Record<string, unknown>;
  in_force: Record<string, unknown>;
  approved: { version: number; capabilities: Record<string, unknown>; approved_at: string; by: string } | null;
  pending: PendingCapabilities | null;
  net: { declared: string[]; waiting: string[]; grants: { host: string; granted_at: string }[]; granted: boolean };
  sample: { granted: boolean; granted_at: string | null };
  db_read: { source_id: string; title: string | null; granted_at: string }[];
};

/** One row. `revoke` is a request that takes the permission back, `"local"`
 * for a grant this browser keeps (revoked through the page's capability
 * host), or null where there is nothing to take back. */
export type PermissionRow = { key: string; label: string; state: string; revoke: { method: "DELETE"; path: string } | "local" | null };

const since = (iso: string | null) => (iso && !Number.isNaN(Date.parse(iso)) ? `Allowed since ${new Date(iso).toLocaleDateString("en-US", { month: "short", day: "numeric", year: "numeric", timeZone: "UTC" })}` : "Allowed");

/** Rows for `p`, the ones waiting for approval first. `comments` is this
 * viewer's state of the page's comments, null where the page has none. */
export function permissionRows(p: PermissionsView, comments: PermissionState | null): PermissionRow[] {
  const base = `/api/artifacts/${p.id}`;
  const rows: PermissionRow[] = [];
  if (p.pending) for (const i of pendingItems(p.pending)) rows.push({ key: `pending:${i.name}`, label: i.label[0].toUpperCase() + i.label.slice(1), state: "Waiting for approval", revoke: null });

  const hosts = p.net.declared;
  if (hosts.length) {
    const granted = p.net.grants.filter(g => hosts.includes(g.host));
    const label = `${capabilityTitle("net")} (${hosts.join(", ")})`;
    if (p.net.granted) rows.push({ key: "net", label, state: since(granted[0]?.granted_at ?? null), revoke: { method: "DELETE", path: `${base}/net-grants` } });
    else if (granted.length) rows.push({ key: "net", label, state: `Allowed for ${granted.length} of ${hosts.length} hosts`, revoke: { method: "DELETE", path: `${base}/net-grants` } });
    else rows.push({ key: "net", label, state: "Asks on first use", revoke: null });
  }
  if (p.sample.granted) rows.push({ key: "sample", label: capabilityTitle("sample"), state: since(p.sample.granted_at), revoke: { method: "DELETE", path: `${base}/sample-grant` } });
  else if (Object.hasOwn(p.in_force, "sample")) rows.push({ key: "sample", label: capabilityTitle("sample"), state: "Asks on first use", revoke: null });
  for (const g of p.db_read) rows.push({ key: `db:${g.source_id}`, label: `${g.title ?? g.source_id}'s data`, state: since(g.granted_at), revoke: { method: "DELETE", path: `${base}/db-grants/${encodeURIComponent(g.source_id)}` } });
  if (p.approved) {
    for (const name of Object.keys(p.approved.capabilities)) {
      if (name === "net" || name === "sample") continue;
      rows.push({ key: `approved:${name}`, label: capabilityTitle(name), state: `Approved for v${p.approved.version}`, revoke: { method: "DELETE", path: `${base}/capability-approval/${encodeURIComponent(name)}` } });
    }
  }
  if (comments === "granted") rows.push({ key: "comments", label: "Comments by the page", state: "Allowed on this device", revoke: "local" });
  else if (comments === "prompt") rows.push({ key: "comments", label: "Comments by the page", state: "Asks on first use", revoke: null });
  return rows;
}
