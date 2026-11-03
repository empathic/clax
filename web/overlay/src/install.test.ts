import { afterEach, describe, expect, it, vi } from "vitest";
import { OVERLAY_TAG } from "../../bridge/src/anchor";
import { DEFAULT_WORDING, INSTALL_META, fetchInstall, installIdOf, settingsOf, type PublicInstallConfig } from "./install";
import { overlay } from "./main";
import { REPORTS_META } from "./model";

const INSTANCE = "https://clax.example";
const CFG: PublicInstallConfig = {
  install_id: "i1", name: "Changelist",
  queue: { id: "q1", name: "Changelist bugs", reporters_see: "own", page: "/queues/q1", reporter_page: "/reporter/q1" },
  config: { product_name: "Acme", report_label: "Feedback", queue_label: "Sends to", reports_button: false, picker: false, brand_color: "#00aa77" },
};
const meta = (name: string, content: string) => {
  const m = document.createElement("meta");
  m.name = name; m.content = content;
  document.head.append(m);
};
const answer = (body: unknown, status = 200) => vi.fn(async () => new Response(JSON.stringify(body), { status }));

afterEach(() => {
  document.head.replaceChildren();
  for (const el of document.querySelectorAll(OVERLAY_TAG)) el.remove();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("installIdOf", () => {
  it("is the option, else the clax-install meta, else none", () => {
    expect(installIdOf(document)).toBeNull();
    meta(INSTALL_META, " i9 ");
    expect(installIdOf(document)).toBe("i9");
    expect(installIdOf(document, "i1")).toBe("i1");
    expect(() => installIdOf(document, "")).toThrow(TypeError);
  });
});

describe("fetchInstall", () => {
  it("fetches the config without credentials, and is null when it cannot", async () => {
    const f = answer(CFG);
    expect(await fetchInstall(INSTANCE, "i/1", f)).toEqual(CFG);
    const [url, init] = f.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("https://clax.example/api/installs/i%2F1/config");
    expect(init).toMatchObject({ mode: "cors", credentials: "omit", cache: "no-cache" });
    expect(await fetchInstall(INSTANCE, "i1", answer({ error: {} }, 404))).toBeNull();
    expect(await fetchInstall(INSTANCE, "i1", answer({ nope: 1 }))).toBeNull();
    expect(await fetchInstall(INSTANCE, "i1", vi.fn(async () => { throw new TypeError("network"); }))).toBeNull();
    const slow = vi.fn((_u: string, init?: RequestInit) => new Promise<Response>((_, no) => init!.signal!.addEventListener("abort", () => no(new Error("aborted")))));
    expect(await fetchInstall(INSTANCE, "i1", slow as typeof fetch, 5)).toBeNull();
  });
});

describe("settingsOf", () => {
  it("takes the install's config over the page's options and metas", () => {
    meta(REPORTS_META, "button");
    const s = settingsOf(CFG, { instance: `${INSTANCE}/` }, document);
    expect(s).toEqual({
      queue: "q1", reports: "off", picker: false,
      wording: { productName: "Acme", reportLabel: "Feedback", queueLabel: "Sends to" },
      queueInfo: { name: "Changelist bugs", page: "https://clax.example/queues/q1", reporterPage: "https://clax.example/reporter/q1" },
      brandColor: "#00aa77",
    });
  });

  it("falls back to the page's options and metas without a config", () => {
    meta(REPORTS_META, "off");
    expect(settingsOf(null, { instance: INSTANCE, queue: "q2" }, document)).toEqual({
      queue: "q2", reports: "off", picker: true, wording: DEFAULT_WORDING, queueInfo: null, brandColor: null,
    });
  });

  it("keeps the page's queue when it names another, and then names no queue", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const s = settingsOf(CFG, { instance: INSTANCE, queue: "q2" }, document);
    expect([s.queue, s.queueInfo]).toEqual(["q2", null]);
    expect(warn).toHaveBeenCalled();
  });

  it("uses the overlay's own words for wording left out", () => {
    const s = settingsOf({ ...CFG, config: { ...CFG.config, product_name: null, report_label: null, queue_label: "  ", brand_color: "red" } }, { instance: INSTANCE }, document);
    expect(s.wording).toEqual(DEFAULT_WORDING);
    expect(s.brandColor).toBeNull();
  });
});

describe("overlay() with an install", () => {
  const auth = async () => { throw new Error("no backend"); };
  const button = (text: string) => [...document.querySelector(OVERLAY_TAG)!.shadowRoot!.querySelectorAll("button")].find(b => b.textContent?.includes(text));

  it("starts once the config is fetched, with only the install's ID", async () => {
    meta(INSTALL_META, "i1");
    vi.stubGlobal("fetch", answer(CFG));
    const h = overlay({ instance: INSTANCE, auth });
    expect(button("Feedback")).toBeUndefined();
    await vi.waitFor(() => expect(button("Feedback")).toBeDefined());
    expect(button("Your reports")).toBeUndefined();
    h.close();
  });

  it("starts on the page's options when the config cannot be fetched", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.stubGlobal("fetch", answer({}, 404));
    const h = overlay({ instance: INSTANCE, queue: "q1", install: "i1", auth });
    await vi.waitFor(() => expect(button("Report")).toBeDefined());
    expect(button("Your reports")).toBeDefined();
    h.close();
  });

  it("needs a queue without an install", () => {
    expect(() => overlay({ instance: INSTANCE, auth } as never)).toThrow(/queue/);
  });
});
