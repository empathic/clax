// What the gallery's components show: reports in each state the overlay's
// panel draws, and people writing.
import type { Report } from "../sdk/src/types";

/** LiveWriters.svelte's LiveWriter (tsc does not read a .svelte module's types; svelte-check checks the prop). */
type LiveWriter = { person_id: string; name: string; color: number; text: string; typing: boolean };

const anchor = { kind: "element", selector: "main > form button.submit", name: "Place order" };

function report(id: string, bodies: string[], over: Partial<Report> = {}): Report {
  return {
    id, queue_id: "q1", artifact: { id: "a1", external_id: "checkout", title: "Checkout", url: "https://store.example/checkout", metadata: null, metadata_version: 0, revision: null },
    anchor, status: "open", state: "open", reporter_id: "u1", metadata: null, metadata_version: 0, has_clip: false, claim: null, created_at: "2026-11-02T10:00:00Z",
    comments: bodies.map((body, i) => ({ id: `${id}c${i}`, author_kind: i ? "agent" : "reporter", author_name: i ? "Triage agent" : "u1", body, created_at: "2026-11-02T10:00:00Z" })),
    ...over,
  };
}

export const REPORTS: Report[] = [
  report("r1", ["The Place order button does nothing on a phone"], {}),
  report("r2", ["The total is wrong after a coupon", "Fixed the coupon rounding in 0.6.1"], {
    state: "fix_proposed",
    fixes: [{ n: 1, summary: "Fixed the coupon rounding in 0.6.1", url: "https://git.example/store/pull/12", verify_url: "https://store.example/checkout", proposer_name: "Triage agent", proposer_kind: "agent", created_at: "2026-11-02T10:00:00Z", outcome: "pending" }],
  }),
  report("r3", ["The address form loses the postcode"], { state: "verified", status: "resolved", resolve_reason: "fixed" }),
];

export const WRITERS: LiveWriter[] = [
  { person_id: "p1", name: "Mia", color: 2, text: "I think the second paragraph should come first.", typing: true },
  { person_id: "p2", name: "Sam", color: 0, text: "", typing: false },
  { person_id: "p3", name: "Priya", color: 3, text: "Agreed — and the chart needs a legend.", typing: false },
];
