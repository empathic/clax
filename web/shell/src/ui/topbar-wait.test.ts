import { describe, expect, it, vi } from "vitest";
import { ID, artifact, artifactViewHooks, mountView, waitFor } from "../test/artifact-view";
import { MOUNT_TIMEOUT_MS } from "../test/timeouts";

// The shell's clock, with each timer it is asked for kept here to be run by
// the test (it never falls due by itself).
const timers: { ms: number; fn: () => void }[] = [];
vi.mock("../clock", async orig => ({
  ...(await orig<typeof import("../clock")>()),
  after: (ms: number, fn: () => void) => { const t = { ms, fn }; timers.push(t); return () => { timers.splice(timers.indexOf(t), 1); }; },
}));

describe("the artifact view's top bar without the bootstrap block", { timeout: MOUNT_TIMEOUT_MS }, () => {
  artifactViewHooks();

  it("holds no frame for what decides its optional controls, and its controls at most BAR_WAIT_MS past the view's being ready", async () => {
    // ClaxBot's runs of the artifact never answer; everything else does.
    const asked: string[] = [];
    const answer = (url: string): Promise<Response> => {
      asked.push(url);
      if (url.startsWith("/api/bot/runs")) return new Promise(() => {});
      if (url === "/api/token") return Promise.resolve(new Response(JSON.stringify({ token: "tk" })));
      if (url.endsWith("/remotes")) return Promise.resolve(new Response(JSON.stringify({ links: [], remotes: [] })));
      if (url.startsWith("/_app/feedback/config")) return Promise.resolve(new Response(JSON.stringify({ enabled: false })));
      return Promise.resolve(new Response(JSON.stringify(artifact(1))));
    };
    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL) => answer(String(input))));
    const { BAR_WAIT_MS, startDeciders } = await import("../view/deciders.svelte");
    startDeciders(ID, null);
    (await import("./more-menu.svelte")).loadMoreMenu();
    const view = await mountView(answer);
    // The frame is in, though the runs are not.
    await waitFor(() => view.root.querySelector("iframe.frame"), "the frame");
    expect(asked.some(u => u.startsWith("/api/bot/runs"))).toBe(true);
    // The controls are laid out, hidden, until the bar's wait ends.
    const cap = await waitFor(() => timers.find(t => t.ms === BAR_WAIT_MS), "the bar's wait");
    expect(view.root.querySelector(".topbar .tools.deciding")).not.toBeNull();
    cap.fn();
    await waitFor(() => view.root.querySelector(".topbar .tools:not(.deciding)"), "the bar's controls");
    expect(view.root.querySelector(".topbar .tools")!.getAttribute("aria-busy")).toBeNull();
    expect(view.root.querySelector(".topbar button.comment")).not.toBeNull();
  });
});
