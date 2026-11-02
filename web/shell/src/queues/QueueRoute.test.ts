// A queue's own page and a report's own page, over the shell's session
// routes (fetch stubbed) and the page's stream's `queues` ping.
import { cleanup, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { EventStream } from "../stream";
import QueueRoute from "./QueueRoute.svelte";

const QID = "01M66XHDKTMTCVB95FE9BQN04V";
const TID = "01J9AAAAAAAAAAAAAAAAAAAAAA";
const queue = { id: QID, name: "Changeling feedback", origins: ["http://127.0.0.1:7700"], reporters_see: "own", created_at: "2026-11-01T00:00:00Z", role: "admin" };
const report = (body: string, id = TID) => ({
  id, queue_id: QID, status: "open", state: "open", reporter_id: null, metadata: null, metadata_version: 0, has_clip: false, claim: null,
  artifact: { id: "a1", external_id: "/", title: "Dashboard", url: "http://127.0.0.1:7700/", metadata: null, metadata_version: 0, revision: null },
  anchor: { kind: "artifact" }, comments: [{ id: "c1", author_kind: "reporter", author_name: "Alex", body, created_at: "2026-11-02T10:00:00Z" }], created_at: "2026-11-02T10:00:00Z",
});
let reports = [report("Save does nothing")];
const json = (b: unknown, status = 200) => new Response(JSON.stringify(b), { status, headers: { "content-type": "application/json" } });
function at(path: string) {
  history.replaceState(null, "", path);
  vi.stubGlobal("fetch", vi.fn(async (u: string) => {
    if (u === "/api/queues") return json({ queues: [queue] });
    if (u === `/api/queues/${QID}/reports`) return json({ reports, next: null });
    if (u === `/api/queues/${QID}/reports/${TID}`) return json({ report: reports[0] });
    return json({ error: { code: "not_found", message: "Not found." } }, 404);
  }));
}
/** A stream whose `ping` sends the `queues` event to what watches it. */
function stream() {
  const fs = new Set<(e: { type: string }) => void>();
  return { s: { watch: (_t: string[], f: (e: { type: string }) => void) => { fs.add(f); return () => fs.delete(f); } } as unknown as EventStream, ping: () => fs.forEach(f => f({ type: "queues" })) };
}
afterEach(() => { cleanup(); vi.unstubAllGlobals(); reports = [report("Save does nothing")]; history.replaceState(null, "", "/"); });

describe("QueueRoute", () => {
  it("shows a queue's own page, titled by its name, its reports linking to their own pages, live", async () => {
    at(`/queues/${QID}`);
    const st = stream();
    render(QueueRoute, { props: { stream: st.s } });
    const link = await screen.findByRole("link", { name: /^Save does nothing/ });
    expect(link.getAttribute("href")).toBe(`/queues/${QID}/${TID}`);
    expect(document.title).toBe("Changeling feedback · Clax");
    reports = [...reports, report("Second", "01J9BBBBBBBBBBBBBBBBBBBBBB")];
    st.ping();
    await screen.findByRole("link", { name: /^Second/ });
  });

  it("shows a report's own page with the way back to its queue", async () => {
    at(`/queues/${QID}/${TID}`);
    render(QueueRoute, { props: {} });
    expect(await screen.findByRole("heading", { name: "Save does nothing" })).toBeTruthy();
    await waitFor(() => expect(screen.getByRole("link", { name: "← Changeling feedback" }).getAttribute("href")).toBe(`/queues/${QID}`));
  });

  it("says so for a queue that is not the viewer's", async () => {
    at("/queues/01M66XHDKTMTCVB95FE9BQN0ZZ");
    render(QueueRoute, { props: {} });
    expect(await screen.findByText(/No queue of yours has this ID/)).toBeTruthy();
  });
});
