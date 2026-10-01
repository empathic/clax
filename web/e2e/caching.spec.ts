import { test, expect, type Response } from "@playwright/test";
import { openArtifact, publish, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: pages are revalidated and the frame loads the bridge at its versioned URL`, async ({ page }) => {
    const { artifact } = await publish(d.base, d.token, `Caching ${mode}`, { "index.html": "<p id=\"hi\">hi</p>" });
    const seen: Response[] = [];
    page.on("response", r => { seen.push(r); });
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(frame.locator("#hi")).toHaveText("hi");
    await expect.poll(() => frame.evaluate(() => typeof (window as unknown as { claude?: unknown }).claude)).toBe("object");

    const shell = seen.find(r => new URL(r.url()).pathname === `/a/${artifact.id}`)!;
    expect(shell.headers()["cache-control"]).toBe("private, no-cache");
    expect(shell.headers()["etag"]).toBeTruthy();

    const doc = seen.find(r => r.url() === frame.url())!;
    expect(doc.headers()["cache-control"]).toBe("no-cache");
    expect(doc.headers()["etag"]).toBeTruthy();

    const src = await frame.evaluate(() => document.querySelector("script[data-artifact]")!.getAttribute("src"));
    expect(src).toMatch(/^\/_clax\/bridge\.js\?v=[0-9a-f]{12}$/);
    const bridge = seen.find(r => r.url().endsWith(src!))!;
    expect(bridge.status()).toBe(200);
    // The e2e daemon is a debug build (`cargo run`), which reads the bridge from
    // disk and so never serves it immutable; release builds do (api_content.rs).
    expect(bridge.headers()["cache-control"]).toBe("no-cache");
    expect(bridge.headers()["etag"]).toBeTruthy();

    const bare = await fetch(`${d.base}/_clax/bridge.js`);
    expect(bare.headers.get("cache-control")).toBe("no-cache");
  });
}
