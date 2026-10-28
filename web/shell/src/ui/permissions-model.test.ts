import { describe, expect, it } from "vitest";
import { type PermissionsView, permissionRows } from "./permissions-model";

const ID = "7q3k9mzx2b4t";
const view = (over: Partial<PermissionsView> = {}): PermissionsView => ({
  id: ID,
  version: 3,
  declared: {},
  in_force: {},
  approved: null,
  pending: null,
  net: { declared: [], waiting: [], grants: [], granted: false },
  sample: { granted: false, granted_at: null },
  db_read: [],
  ...over,
});
const base = `/api/artifacts/${ID}`;

describe("permissionRows", () => {
  it("rows_list_each_grant_with_its_revoke", () => {
    const rows = permissionRows(view({
      in_force: { net: { hosts: ["api.example.com"] }, sample: {}, shell: {} },
      approved: { version: 3, capabilities: { net: { hosts: ["api.example.com"] }, sample: {}, shell: {} }, approved_at: "2026-10-01T00:00:00Z", by: "owner" },
      net: { declared: ["api.example.com"], waiting: [], grants: [{ host: "api.example.com", granted_at: "2026-10-02T12:00:00Z" }], granted: true },
      sample: { granted: true, granted_at: "2026-10-03T12:00:00Z" },
      db_read: [{ source_id: "srcsrcsrc123", title: "Budget", granted_at: "2026-10-04T12:00:00Z" }],
    }), "granted");
    expect(rows.map(r => r.label)).toEqual(["The internet (api.example.com)", "Claude", "Budget's data", "Clax's controls", "Comments by the page"]);
    expect(rows.map(r => r.revoke)).toEqual([
      { method: "DELETE", path: `${base}/net-grants` },
      { method: "DELETE", path: `${base}/sample-grant` },
      { method: "DELETE", path: `${base}/db-grants/srcsrcsrc123` },
      { method: "DELETE", path: `${base}/capability-approval/shell` },
      "local",
    ]);
    expect(rows[0].state).toBe("Allowed since Oct 2, 2026");
    expect(rows[1].state).toBe("Allowed since Oct 3, 2026");
    expect(rows[3].state).toBe("Approved for v3");
  });

  it("a_declared_host_not_granted_asks_on_first_use", () => {
    const rows = permissionRows(view({ net: { declared: ["api.example.com"], waiting: [], grants: [], granted: false } }), null);
    expect(rows).toEqual([{ key: "net", label: "The internet (api.example.com)", state: "Asks on first use", revoke: null }]);
  });

  it("pending_items_come_first", () => {
    const rows = permissionRows(view({
      pending: { version: 3, capabilities: ["shell"], hosts: [], declaration_sha256: "abc" },
      sample: { granted: true, granted_at: "2026-10-03T12:00:00Z" },
    }), null);
    expect(rows[0]).toMatchObject({ label: "Clax's controls", state: "Waiting for approval", revoke: null });
    expect(rows[1].label).toBe("Claude");
  });
});
