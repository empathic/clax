// Report fixtures for the queue and report views' unit tests.
import type { Report, ReportContext } from "../../../sdk/src/types";

export const QUEUE = { id: "q1", name: "Bugs", origins: ["https://app.example"], reporters_see: "own" as const, created_at: "2026-10-27T10:00:00Z", role: "triage" as const };

export const CONTEXT: ReportContext = {
  v: 1,
  captured_at: "2026-10-29T07:12:03.120Z",
  release: { version: "0.6.0", commit: "f7e862ab0aa1bb2cc3dd", channel: "prod" },
  flags: { claxbot: true, new_editor: "variant-b", beta: false },
  environment: {
    url: "https://app.example/a/1?tab=", route: "/a/:id", title: "Invoice",
    viewport: { w: 1440, h: 900, dpr: 2 }, scroll: { x: 0, y: 340 },
    user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    platform: "macOS", locale: "en-US", time_zone: "America/Los_Angeles", color_scheme: "dark",
  },
  capture: { mode: "screen", method: "dom", omitted: [{ kind: "iframe", rect: { x: 100, y: 50, w: 200, h: 100 } }] },
  app: { repo: "https://github.com/empathic/clax" },
};

/** A report `t<n>` of queue q1, open, with its filing note and no fixes. */
export const report = (n: number, o: Partial<Report> = {}): Report => ({
  id: `t${n}`, queue_id: "q1",
  artifact: { id: "a1", external_id: "e1", title: "Invoice 4821", url: "https://app.example/invoices/4821", metadata: null, metadata_version: 1, revision: null },
  anchor: { kind: "element", name: "Total", selector: "#total" }, status: "open", state: "open", reporter_id: "emp-1",
  metadata: null, metadata_version: 1, has_clip: true,
  comments: [{ id: `c${n}`, author_kind: "reporter", author_name: "emp-1", body: `Report ${n}: the total is wrong`, created_at: "2026-10-29T07:12:04Z" }],
  claim: null, created_at: "2026-10-29T07:12:04Z", context: CONTEXT, ...o,
});
