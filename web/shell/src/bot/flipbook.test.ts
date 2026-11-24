import { afterEach, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { settle } from "../people/fixtures";
import { flush, mount } from "../test/svelte";
import Flipbook from "./Flipbook.svelte";
import { blobUrls } from "../test/blob-urls";

afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); document.body.replaceChildren(); });

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });

it("a_clip_plays_its_frames_in_order", async () => {
  const manifest = { width: 390, height: 844, frames: [{ item: "f1", at_ms: 0 }, { item: "f2", at_ms: 125 }, { item: "f3", at_ms: 500 }] };
  vi.stubGlobal("fetch", vi.fn(async (url: string) => url === "/api/bot/runs/r1/content/clip1"
    ? new Response(JSON.stringify(manifest), { headers: { "content-type": "application/json" } })
    : url.startsWith("/api/bot/runs/r1/content/f")
      ? new Response(url.slice(url.lastIndexOf("/") + 1), { headers: { "content-type": "image/jpeg" } })
      : new Response("", { status: 404 })));
  const blobs = blobUrls();
  const m = mount(Flipbook, { run: "r1", clip: "clip1", label: "Recording 1" });
  await settle();
  const root = document.querySelector<HTMLElement>(".flip")!;
  // Each frame is fetched through the shell's fetch and shown from a blob: URL.
  const shown = () => root.querySelector("img")!.getAttribute("src");
  const urls = new Map(await Promise.all(blobs.made().map(async u => [u, await blobs.text(u)] as const)));
  const frame = () => urls.get(shown()!);
  const play = () => root.querySelector("button")!;
  expect(root.getAttribute("aria-label")).toBe("Recording 1");
  expect(frame()).toBe("f1");
  expect(root.querySelector(".at")!.textContent).toBe("1 / 3");
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  click(play());
  expect(play().textContent).toBe("Pause");
  flush(() => { vi.advanceTimersByTime(124); });
  expect(frame()).toBe("f1");
  flush(() => { vi.advanceTimersByTime(1); });
  expect(frame()).toBe("f2");
  flush(() => { vi.advanceTimersByTime(375); });
  expect(frame()).toBe("f3");
  expect(root.querySelector(".at")!.textContent).toBe("3 / 3");
  // At its end it stops; Play starts it again from the first frame.
  expect(play().textContent).toBe("Play");
  click(play());
  expect(frame()).toBe("f1");
  m.unmount();
  expect(blobs.revoked.sort()).toEqual(blobs.made().sort());
  blobs.restore();
});
