// The quickstart page's own script (web/quickstart/index.html), run under
// jsdom with a stubbed `window.claude`: a signed-in person on clax.dev has
// step 1 ticked and is told so. The page runs on its artifact's own site,
// which sends no referrer, so clax.dev and the page's address in Clax are
// known from the `shell` capability's `address()`.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const html = readFileSync(join(__dirname, "../quickstart/index.html"), "utf8");
const doc = new DOMParser().parseFromString(html, "text/html");
const script = [...doc.querySelectorAll("script")].find((s) => !s.src)!.textContent!;
const body = (() => {
  const b = doc.body.cloneNode(true) as HTMLElement;
  b.querySelectorAll("script").forEach((s) => s.remove());
  return b.innerHTML;
})();

/** A viewer the `user` capability names: a person (`guest: false`) or a guest. */
const userOf = (guest: boolean) => ({
  me: async () => ({ id: "u1", name: "ada" }),
  profiles: async () => ({ u1: { name: "Ada Lovelace", guest } }),
  id: async () => "u1",
});

/** A `shell` whose `address()` answers `address` (rejecting when it is an Error). */
const shellAt = (address: unknown) => ({
  can: async () => false,
  highlight: async () => false,
  commentMode: async () => false,
  address: async () => { if (address instanceof Error) throw address; return address; },
});

/** Runs the page for a viewer `user` names, with `shell` (null: not served) and `referrer`. */
function load(shell: unknown, user = userOf(false), referrer = "") {
  document.body.innerHTML = body;
  // The site's no-referrer policy: a page that read the referrer would find nothing, or a spoof.
  Object.defineProperty(document, "referrer", { value: referrer, configurable: true });
  vi.stubGlobal("claude", {
    use: async (name: string) => {
      if (name === "user") return user;
      if (name === "shell") return shell;
      throw new Error(`no ${name}`);
    },
  });
  new Function(script)();
}

const at = (origin: string) => shellAt({ origin, url: `${origin}/a/abcd1234efgh` });

const toast = () => document.getElementById("toast")!;
const saved = () => JSON.parse(localStorage.getItem("clax-quickstart") ?? "null");
/** Lets the page's capability calls settle: they are promises alone, no timers. */
const settle = async () => { for (let i = 0; i < 100; i++) await Promise.resolve(); };

beforeEach(() => {
  localStorage.clear();
  sessionStorage.clear();
  vi.stubGlobal("matchMedia", () => ({ matches: true }));
  vi.stubGlobal("ResizeObserver", class { observe() {} });
  Element.prototype.scrollIntoView = () => {};
});
afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); history.replaceState(null, "", location.pathname); });

describe("the quickstart notices a sign-in", () => {
  it("ticks_sign_in_and_says_so_for_a_person_on_clax_dev", async () => {
    load(at("https://clax.dev"));
    await settle();
    expect(toast().hidden).toBe(false);
    expect(toast().textContent).toBe("Signed in as Ada Lovelace — on to step 2");
    expect(saved().done.signin).toBe(true);
    expect(document.querySelector('[data-id="signin"] .fab')!.classList.contains("on")).toBe(true);
  });

  it("is_hosted_under_clax_dev_only", async () => {
    for (const [origin, hosted] of [["https://eu.clax.dev", true], ["https://clax.dev.evil.example", false], ["https://notclax.dev", false], ["http://localhost:7480", false], ["https://clax.example.com", false]] as const) {
      localStorage.clear();
      load(at(origin));
      await settle();
      expect(saved()?.done?.signin === true, origin).toBe(hosted);
      document.body.replaceChildren();
    }
  });

  it("does_not_trust_the_referrer", async () => {
    // No shell answer: a referrer that names clax.dev decides nothing.
    load(null, userOf(false), "https://clax.dev/a/abcd1234efgh");
    await settle();
    expect(toast().hidden).toBe(true);
    expect(saved()?.done?.signin).toBeUndefined();
  });

  it("leaves_sign_in_alone_on_a_local_clax", async () => {
    load(at("http://localhost:7480"));
    await settle();
    expect(toast().hidden).toBe(true);
    expect(saved()?.done?.signin).toBeUndefined();
  });

  it("leaves_sign_in_alone_for_a_guest", async () => {
    load(at("https://clax.dev"), userOf(true));
    await settle();
    expect(toast().hidden).toBe(true);
    expect(saved()?.done?.signin).toBeUndefined();
  });

  it("leaves_sign_in_alone_when_the_shell_cannot_say", async () => {
    for (const shell of [shellAt(null), shellAt(new Error("capability_removed")), shellAt({ origin: 1 })]) {
      load(shell);
      await settle();
      expect(saved()?.done?.signin).toBeUndefined();
      document.body.replaceChildren();
    }
  });
});

describe("the attach step names the page's address", () => {
  const prompt = () => [...document.querySelectorAll(".cmd pre")].map((p) => p.textContent).find((t) => t?.startsWith("Watch ")) ?? null;

  it("gives_the_address_the_shell_says", async () => {
    location.hash = "#attach";
    // A guest: a person's sign-in would move the open step on to the next.
    load(at("https://clax.dev"), userOf(true));
    await settle();
    expect(prompt()).toBe("Watch https://clax.dev/a/abcd1234efgh for my comments.");
  });

  it("names_the_page_by_its_title_without_an_address", async () => {
    location.hash = "#attach";
    load(null);
    await settle();
    // jsdom's host is no artifact's site, so the page has neither address nor ID.
    expect(prompt()).toBe("Watch the Clax Quickstart page (this page) for my comments.");
  });
});

describe("the reel", () => {
  const fig = () => document.getElementById("reel")!;
  const watch = () => document.getElementById("reel-show")!;
  const loaded = () => fig().querySelector("video")!.dispatchEvent(new Event("loadedmetadata"));

  it("shows_only_once_the_video_loads", () => {
    load(null);
    expect(fig().hidden).toBe(true);
    expect(watch().hidden).toBe(true);
    loaded();
    expect(fig().hidden).toBe(false);
    expect(watch().hidden).toBe(true);
  });

  it("stays_hidden_once_hidden_and_comes_back_on_request", () => {
    load(null);
    loaded();
    (document.getElementById("reel-hide") as HTMLButtonElement).click();
    expect(fig().hidden).toBe(true);
    expect(watch().hidden).toBe(false);
    // A later visit keeps it hidden, offering it back.
    load(null);
    loaded();
    expect(fig().hidden).toBe(true);
    expect(watch().hidden).toBe(false);
    watch().click();
    expect(fig().hidden).toBe(false);
    expect(localStorage.getItem("clax-quickstart-reel-hidden")).toBeNull();
  });

  it("works_without_storage", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("blocked"); });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("blocked"); });
    try {
      load(null);
      loaded();
      expect(fig().hidden).toBe(false);
      (document.getElementById("reel-hide") as HTMLButtonElement).click();
      expect(fig().hidden).toBe(true);
    } finally { vi.restoreAllMocks(); }
  });
});
