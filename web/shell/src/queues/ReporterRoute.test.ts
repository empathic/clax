// A reporter's view of its queue and of one report, over the view's cookie
// routes (fetch stubbed): what each setting shows of who filed a report, the
// reply, and an ended view.
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import ReporterRoute from "./ReporterRoute.svelte";
import { filedBy, reporterRouteOf } from "./reporter-api";

const QID = "01M66XHDKTMTCVB95FE9BQN04V";
const MINE = "01J9AAAAAAAAAAAAAAAAAAAAAA";
const THEIRS = "01J9BBBBBBBBBBBBBBBBBBBBBB";
const report = (id: string, body: string, submitter?: { name: string | null; you: boolean }, filed = "2026-11-02T10:00:00Z") => ({
  id, queue_id: QID, status: "open", state: "open", reporter_id: submitter && !submitter.you ? null : "emp-a", metadata: null, metadata_version: 0, has_clip: false, claim: null,
  artifact: { id: "a1", external_id: "/", title: "Dashboard", url: "http://127.0.0.1:7700/", metadata: null, metadata_version: 0, revision: null },
  anchor: { kind: "artifact" }, created_at: filed,
  comments: [{ id: `c-${id}`, author_kind: "reporter", author_name: submitter?.name ?? "alice-gh", body, created_at: filed }],
  ...(submitter ? { submitter } : {}),
});
const json = (b: unknown, status = 200) => new Response(JSON.stringify(b), { status, headers: { "content-type": "application/json" } });
const base = `/reporter/${QID}/api`;

function at(path: string, see: "own" | "all", reports: unknown[], answer?: (u: string, init?: RequestInit) => Response | undefined) {
  history.replaceState(null, "", path);
  const f = vi.fn(async (u: string, init?: RequestInit) => {
    const a = answer?.(u, init);
    if (a) return a;
    if (u === `${base}/queue`) return json({ queue: { id: QID, name: "Changeling feedback", reporters_see: see } });
    if (u === `${base}/reports`) return json({ reports, next: null });
    const one = (reports as { id: string }[]).find(r => u === `${base}/reports/${r.id}`);
    if (one) return json({ report: one });
    return json({ error: { code: "not_found", message: "Not found." } }, 404);
  });
  vi.stubGlobal("fetch", f);
  return f;
}
afterEach(() => { cleanup(); vi.unstubAllGlobals(); history.replaceState(null, "", "/"); });

describe("reporterRouteOf", () => {
  it("reads the queue and report from the path, and nothing else", () => {
    expect(reporterRouteOf(`/reporter/${QID}`)).toEqual({ qid: QID, tid: null });
    expect(reporterRouteOf(`/reporter/${QID}/${MINE}`)).toEqual({ qid: QID, tid: MINE });
    expect(reporterRouteOf(`/reporter/${QID}/enter`)).toBeNull();
    expect(reporterRouteOf(`/reporter/${QID}/api/reports`)).toBeNull();
    expect(reporterRouteOf("/queues/x")).toBeNull();
  });
  it("says who filed a report only when the queue shows every report", () => {
    expect(filedBy(report(MINE, "x") as never)).toBeNull();
    expect(filedBy(report(MINE, "x", { name: "alice-gh", you: true }) as never)).toBe("You");
    expect(filedBy(report(THEIRS, "x", { name: "bob-gh", you: false }) as never)).toBe("bob-gh");
    expect(filedBy(report(THEIRS, "x", { name: null, you: false }) as never)).toBe("Another reporter");
  });
});

describe("ReporterRoute", () => {
  it("under “Their own reports” lists the reporter's own, with no submitter", async () => {
    at(`/reporter/${QID}`, "own", [report(MINE, "Save does nothing")]);
    render(ReporterRoute);
    expect(await screen.findByRole("heading", { name: "Your reports in Changeling feedback" })).toBeTruthy();
    const link = await screen.findByRole("link", { name: /^Save does nothing/ });
    expect(link.getAttribute("href")).toBe(`/reporter/${QID}/${MINE}`);
    expect(link.textContent).not.toMatch(/filed by|yours/);
    expect(screen.getByText("This queue shows you only the reports you filed.")).toBeTruthy();
  });

  it("under “Every report in the queue” names who filed each, the reader's own as yours, newest first", async () => {
    at(`/reporter/${QID}`, "all", [
      report(MINE, "Save does nothing", { name: "alice-gh", you: true }, "2026-11-02T10:00:00Z"),
      report(THEIRS, "Chart is blank", { name: "bob-gh", you: false }, "2026-11-02T11:00:00Z"),
    ]);
    render(ReporterRoute);
    expect(await screen.findByRole("heading", { name: "Reports in Changeling feedback" })).toBeTruthy();
    const links = await screen.findAllByRole("link");
    const text = links.map(l => l.textContent?.replace(/\s+/g, " ") ?? "");
    expect(text).toHaveLength(2);
    expect(text[0]).toMatch(/^Chart is blank.*· filed by bob-gh ·/);
    expect(text[1]).toMatch(/^Save does nothing.*· yours ·/);
    expect(text.join(" ")).not.toMatch(/emp-|alice-gh/);
  });

  it("shows a report, who filed it, and sends a reply as the reporter", async () => {
    let r = report(THEIRS, "Chart is blank", { name: "bob-gh", you: false });
    const f = at(`/reporter/${QID}/${THEIRS}`, "all", [r], (u, init) => {
      if (u === `${base}/reports/${THEIRS}/comments` && init?.method === "POST") {
        const body = JSON.parse(String(init.body)).body as string;
        r = { ...r, comments: [...r.comments, { id: "c2", author_kind: "reporter", author_name: "alice-gh", body, created_at: "2026-11-02T12:00:00Z" }] };
        return json({ report: r });
      }
      return undefined;
    });
    render(ReporterRoute);
    expect(await screen.findByRole("heading", { name: "Chart is blank" })).toBeTruthy();
    expect(await screen.findByText(/filed by bob-gh/)).toBeTruthy();
    await waitFor(() => expect(screen.getByRole("link", { name: "← Reports in Changeling feedback" }).getAttribute("href")).toBe(`/reporter/${QID}`));
    expect(screen.queryByRole("button", { name: "Resolve" })).toBeNull();
    await fireEvent.input(screen.getByLabelText("Reply"), { target: { value: "Same here" } });
    await fireEvent.click(screen.getByRole("button", { name: "Send reply" }));
    expect(await screen.findByText("Reply sent.")).toBeTruthy();
    expect(screen.getByText("Same here")).toBeTruthy();
    const post = f.mock.calls.find(([u, i]) => u === `${base}/reports/${THEIRS}/comments` && i?.method === "POST");
    expect((post![1]!.headers as Record<string, string>)["content-type"]).toBe("application/json");
  });

  it("says to open it from the app again once the view has ended", async () => {
    at(`/reporter/${QID}`, "own", [], u => (u === `${base}/queue` ? json({ error: { code: "unauthorized", message: "unauthorized" } }, 401) : undefined));
    render(ReporterRoute);
    expect(await screen.findByRole("heading", { name: "This view has ended" })).toBeTruthy();
  });
});
