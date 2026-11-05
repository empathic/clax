import { afterEach, describe, expect, it, vi } from "vitest";
import { OVERLAY_TAG } from "../../bridge/src/anchor";
import { DEFAULT_WORDING, INSTALL_META, fetchInstall, installIdOf, settingsOf, type PublicInstallConfig } from "./install";
import { overlay } from "./main";
import { REPORTS_META } from "./model";
import { inkOn } from "./ribbon";

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
    const slow = vi.fn((_u: string, req?: RequestInit) => new Promise<Response>((_, no) => req!.signal!.addEventListener("abort", () => no(new Error("aborted")))));
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
      trigger: null, fKey: false, ribbon: null,
    });
  });

  it("takes how the page opens feedback from the install, over the page's own options", () => {
    const page = { instance: INSTANCE, trigger: ".mine", fKey: true, ribbon: "Mine" };
    expect(settingsOf({ ...CFG, config: { ...CFG.config, trigger: " .feedback-btn ", f_key: false, ribbon: true, ribbon_text: "Early Access" } }, page, document))
      .toMatchObject({ trigger: ".feedback-btn", fKey: false, ribbon: "Early Access" });
    // The ribbon off, its wording kept for when it is on again; and an older Clax's config, without these.
    expect(settingsOf({ ...CFG, config: { ...CFG.config, ribbon: false, ribbon_text: "Early Access" } }, page, document).ribbon).toBeNull();
    expect(settingsOf(CFG, page, document)).toMatchObject({ trigger: null, fKey: false, ribbon: null });
    // No config: the page's own.
    expect(settingsOf(null, page, document)).toMatchObject({ trigger: ".mine", fKey: true, ribbon: "Mine" });
  });

  it("puts the queue's pages on the origin the config declares, not the instance it was given", () => {
    const s = settingsOf({ ...CFG, origin: "https://clax.tail556f5b.ts.net" }, { instance: "https://phobos.tail556f5b.ts.net" }, document);
    expect(s.queueInfo).toEqual({
      name: "Changelist bugs",
      page: "https://clax.tail556f5b.ts.net/queues/q1",
      reporterPage: "https://clax.tail556f5b.ts.net/reporter/q1",
    });
    const bad = settingsOf({ ...CFG, origin: "javascript:alert(1)" }, { instance: INSTANCE }, document);
    expect(bad.queueInfo?.page).toBe("https://clax.example/queues/q1");
  });

  it("falls back to the page's options and metas without a config", () => {
    meta(REPORTS_META, "off");
    expect(settingsOf(null, { instance: INSTANCE, queue: "q2" }, document)).toEqual({
      queue: "q2", reports: "off", picker: true, wording: DEFAULT_WORDING, queueInfo: null, brandColor: null, trigger: null, fKey: false, ribbon: null,
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

  const ribbonOf = () => [...document.querySelectorAll(OVERLAY_TAG)].map(h => h.shadowRoot!.querySelector<HTMLElement>(".ribbon")).find(Boolean) ?? null;
  const opening = { ...CFG, config: { ...CFG.config, reports_button: true, trigger: ".fb", f_key: true, ribbon: true, ribbon_text: "Early Access" } };
  const started = async (o: object = {}, cfg: PublicInstallConfig = opening) => {
    vi.stubGlobal("fetch", answer(cfg));
    const h = overlay({ instance: INSTANCE, install: "i1", auth, ...o } as never);
    await vi.waitFor(() => expect(ribbonOf() ?? document.querySelector(OVERLAY_TAG)!.shadowRoot!.querySelector(".cx")).not.toBeNull());
    return h;
  };
  const modeBar = () => [...document.querySelectorAll(OVERLAY_TAG)].map(h => h.shadowRoot!.querySelector(".modebar")).find(Boolean) ?? null;
  const press = (key: string, at: EventTarget = document.body, init: KeyboardEventInit = {}) => at.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, composed: true, cancelable: true, ...init }));

  it("a trigger the page renders later opens it, with no corner button, and focus returns to it", async () => {
    const h = await started();
    expect(button("Report")).toBeUndefined();
    const later = document.createElement("button");
    later.className = "fb";
    later.textContent = "Feedback";
    document.body.append(later);
    later.click();
    await vi.waitFor(() => expect(modeBar()).not.toBeNull());
    press("Escape", later);
    await vi.waitFor(() => expect(modeBar()).toBeNull());
    expect(document.activeElement).toBe(later);
    expect(later.tagName).toBe("BUTTON");
    later.remove();
    h.close();
  });

  it("an element the page marks opens it, even when the config cannot be fetched, and a page's stopPropagation does not hide it", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.stubGlobal("fetch", answer({}, 404));
    const h = overlay({ instance: INSTANCE, queue: "q1", install: "i1", auth });
    await vi.waitFor(() => expect(button("Report")).toBeDefined());
    const b = document.createElement("button");
    b.dataset.feedback = "report";
    b.addEventListener("click", e => e.stopPropagation());
    document.body.append(b);
    b.click();
    await vi.waitFor(() => expect(modeBar()).not.toBeNull());
    b.remove();
    h.close();
  });

  it("F opens it, but not lowercase, not with a modifier, not typed in a field, and not a key the page used", async () => {
    const h = await started();
    const field = document.createElement("input");
    const editable = document.createElement("div");
    editable.contentEditable = "true";
    editable.setAttribute("role", "textbox");
    document.body.append(field, editable);
    press("f");
    press("F", document.body, { metaKey: true });
    press("F", document.body, { ctrlKey: true });
    press("F", field);
    press("F", editable);
    const taken = (e: Event) => e.preventDefault();
    document.body.addEventListener("keydown", taken);
    press("F");
    document.body.removeEventListener("keydown", taken);
    await new Promise(r => setTimeout(r, 0));
    expect(modeBar()).toBeNull();
    press("F");
    await vi.waitFor(() => expect(modeBar()).not.toBeNull());
    field.remove(); editable.remove();
    h.close();
  });

  it("F is off unless the install turns it on", async () => {
    const h = await started({}, { ...opening, config: { ...opening.config, f_key: false } });
    press("F");
    await new Promise(r => setTimeout(r, 0));
    expect(modeBar()).toBeNull();
    h.close();
  });

  it("the ribbon is a real, named button first in the page, focus returns to it, and it goes with the overlay", async () => {
    const before = document.createElement("a");
    before.href = "#x";
    document.body.append(before);
    const h = await started();
    const r = ribbonOf()!;
    expect(r.tagName).toBe("BUTTON");
    expect(r.getAttribute("aria-label")).toBe("Early Access — send feedback");
    expect(r.getAttribute("aria-keyshortcuts")).toBe("Shift+F");
    expect(r.querySelector("text")!.textContent).toBe("EARLY ACCESS");
    expect(r.querySelector("svg")!.getAttribute("aria-hidden")).toBe("true");
    expect(document.body.firstElementChild!.localName).toBe(OVERLAY_TAG);
    r.focus();
    r.click();
    await vi.waitFor(() => expect(modeBar()).not.toBeNull());
    press("Escape", document.body);
    await vi.waitFor(() => expect(modeBar()).toBeNull());
    expect((r.getRootNode() as ShadowRoot).activeElement).toBe(r);
    h.close();
    expect(ribbonOf()).toBeNull();
    expect(document.querySelectorAll(OVERLAY_TAG)).toHaveLength(0);
    before.remove();
  });

  it("the wording is text, never markup", async () => {
    const h = await started({}, { ...opening, config: { ...opening.config, ribbon_text: "<img src=x>" } });
    const r = ribbonOf()!;
    expect(r.querySelector("img")).toBeNull();
    expect(r.querySelector("text")!.textContent).toBe("<IMG SRC=X>");
    h.close();
  });

  it("with nobody signed in it is the ribbon as a label: no token asked, no trigger, no key", async () => {
    const fetched = answer(opening);
    vi.stubGlobal("fetch", fetched);
    const h = overlay({ instance: INSTANCE, install: "i1", canFile: false });
    await vi.waitFor(() => expect(ribbonOf()).not.toBeNull());
    const r = ribbonOf()!;
    expect(r.tagName).toBe("SPAN");
    expect(r.getAttribute("role")).toBe("img");
    expect(r.getAttribute("aria-label")).toBe("Early Access");
    expect(button("Your reports")).toBeUndefined();
    press("F");
    await new Promise(res => setTimeout(res, 0));
    expect(modeBar()).toBeNull();
    expect(fetched).toHaveBeenCalledTimes(1);
    h.close();
  });

  it("a trigger that is no selector is ignored, and the mark still works", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const h = await started({}, { ...opening, config: { ...opening.config, trigger: "button[" } });
    expect(warn).toHaveBeenCalledWith(expect.stringContaining("not a CSS selector"));
    document.body.click();
    const b = document.createElement("button");
    b.dataset.feedback = "report";
    document.body.append(b);
    b.click();
    await vi.waitFor(() => expect(modeBar()).not.toBeNull());
    b.remove();
    h.close();
  });
});

describe("inkOn", () => {
  it("is whichever of black and white reads at 4.5:1 or better", () => {
    const lum = (hex: string) => {
      const c = (at: number) => { const v = parseInt(hex.slice(at, at + 2), 16) / 255; return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; };
      return 0.2126 * c(1) + 0.7152 * c(3) + 0.0722 * c(5);
    };
    const ratio = (a: string, b: string) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
    for (const band of ["#e0532f", "#000000", "#ffffff", "#777777", "#ff7a57", "#0000ff", "#00aa77"]) expect(ratio(band, inkOn(band))).toBeGreaterThanOrEqual(4.5);
  });
});
