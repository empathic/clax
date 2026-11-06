import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import { QUEUE } from "./fixtures";
import Installs from "./Installs.svelte";
import type { Install } from "../../../sdk/src/types";

const LIST = "/api/queues/q1/installs";
const install = (o: Partial<Install> = {}): Install => ({
  id: "i1", queue_id: "q1", name: "Changelist", origins: ["https://app.example"],
  config: { product_name: "Acme", report_label: null, queue_label: null, reports_button: true, picker: false, brand_color: null },
  created_at: "2026-11-03T10:00:00Z", updated_at: "2026-11-03T10:00:00Z", ...o,
});
const click = (el: Element | null | undefined) => flush(() => (el as HTMLElement).click());
const button = (root: Element, text: string) => [...root.querySelectorAll("button")].find(b => b.textContent === text);
const type = (root: Element, name: string, value: string) => flush(() => {
  const el = root.querySelector<HTMLInputElement>(`[name="${name}"]`)!;
  el.value = value;
  el.dispatchEvent(new Event("input", { bubbles: true }));
});
const submit = (root: Element) => flush(() => root.querySelector<HTMLFormElement>("form.edit")!.requestSubmit());

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("Installs", () => {
  it("lists each install with its meta tag, read-only for triage", async () => {
    stubFetch({ [`GET ${LIST}`]: { installs: [install()] } });
    const m = mount(Installs, { queue: QUEUE, admin: false });
    await settle();
    const row = m.root.querySelector('[data-install="i1"]')!;
    expect(row.textContent).toContain("Changelist");
    expect(row.textContent).toContain("https://app.example");
    expect(row.querySelector(".sum")!.textContent).toBe('named "Acme" · Your reports · no element picker');
    expect(row.querySelector("code")!.textContent).toBe('<meta name="clax-install" content="i1">');
    expect(button(m.root, "Edit")).toBeUndefined();
    expect(button(m.root, "New install")).toBeUndefined();
  });

  it("shows each install how to add it to a site, filled from its real values, and announces a copy", async () => {
    stubFetch({ [`GET ${LIST}`]: { installs: [install()] } });
    const writes: string[] = [];
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText: (t: string) => { writes.push(t); return Promise.resolve(); } } });
    const m = mount(Installs, { queue: QUEUE, admin: false });
    await settle();
    const site = m.root.querySelector('[data-site="i1"]')!;
    expect(site.querySelector("summary")!.textContent).toBe("Add it to your site");
    expect([...site.querySelectorAll("[data-way]")].map(w => w.getAttribute("data-way"))).toEqual(["script", "build", "server"]);
    const snip = (id: string) => site.querySelector(`[data-snippet="${id}"]`)!.textContent!;
    const origin = location.origin;
    for (const id of ["script-tag", "sdk", "dts", "own-button"]) {
      expect(snip(id)).toContain(`instance: "${origin}"`);
      expect(snip(id)).toContain('queue: "q1"');
      expect(snip(id)).toContain('install: "i1"');
    }
    expect(snip("script-tag")).toContain(`<script src="${origin}/_clax/overlay.js"></script>`);
    expect(snip("sdk")).toContain('import { loadOverlay, reporterTokenFrom } from "@empathic/clax-client-sdk";');
    expect(snip("dts")).toContain(`"${origin}/_clax/overlay.mjs"`);
    expect(snip("env")).toContain(`CLAX_INSTANCE=${origin}\nCLAX_QUEUE=q1\nCLAX_INSTALL=i1\n`);
    expect(snip("env")).toContain("CLAX_QUEUE_TOKEN=cxq_…");
    expect(snip("route")).toContain('origin: "https://app.example"');
    expect(snip("own-button")).toContain("launcher: false");
    expect(snip("own-button")).toContain("handle.subscribe(s =>");
    expect(site.querySelector(".licence")!.textContent).toContain("LGPL-2.1-only");
    expect(site.querySelector(".licence")!.textContent).toContain("AGPL-3.0-only");

    const live = m.root.querySelector('[aria-live="polite"]')!;
    expect(live.textContent).toBe("");
    const copyBtn = site.querySelector<HTMLButtonElement>('[aria-label="Copy @empathic/clax-client-sdk"]')!;
    expect(copyBtn.tagName).toBe("BUTTON"); // a real button: in the tab order
    click(copyBtn);
    await settle();
    expect(writes).toEqual([snip("sdk")]);
    expect(copyBtn.textContent).toBe("Copied");
    expect(live.textContent).toBe("Copied @empathic/clax-client-sdk.");
  });

  it("says when a copy fails", async () => {
    stubFetch({ [`GET ${LIST}`]: { installs: [install()] } });
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText: () => Promise.reject(new Error("denied")) } });
    const m = mount(Installs, { queue: QUEUE, admin: false });
    await settle();
    click(m.root.querySelector('[aria-label="Copy Your backend\'s environment"]'));
    await settle();
    expect(m.root.querySelector('[aria-live="polite"]')!.textContent).toBe("Could not copy Your backend's environment; select the text instead.");
  });

  it("makes an install, its origins defaulting to the queue's", async () => {
    let made = false;
    const calls = stubFetch({
      [`GET ${LIST}`]: () => ({ installs: made ? [install({ name: "Basis" })] : [] }),
      [`POST ${LIST}`]: () => { made = true; return { install: install({ name: "Basis" }) }; },
    });
    const m = mount(Installs, { queue: QUEUE, admin: true });
    await settle();
    expect(m.root.querySelector(".none")!.textContent).toBe("No installs yet.");
    click(button(m.root, "New install"));
    expect(m.root.querySelector<HTMLTextAreaElement>('[name="install_origins"]')!.value).toBe("https://app.example");
    type(m.root, "install_name", "Basis");
    type(m.root, "queue_label", " Sends to ");
    submit(m.root);
    await settle();
    const post = calls.find(c => c.method === "POST")!;
    expect(post.body).toEqual({
      name: "Basis", origins: ["https://app.example"],
      config: { product_name: null, report_label: null, queue_label: "Sends to", reports_button: true, picker: true, brand_color: null },
    });
    expect(m.root.querySelector('[data-install="i1"]')!.textContent).toContain("Basis");
    expect(m.root.querySelector("form.edit")).toBeNull();
  });

  it("edits and says a refusal", async () => {
    const calls = stubFetch({
      [`GET ${LIST}`]: { installs: [install()] },
      [`PATCH ${LIST}/i1`]: [400, { error: { code: "invalid_install", message: "the accent colour (brand_color) is #rrggbb" } }],
    });
    const m = mount(Installs, { queue: QUEUE, admin: true });
    await settle();
    click(button(m.root, "Edit"));
    expect(m.root.querySelector<HTMLInputElement>('[name="product_name"]')!.value).toBe("Acme");
    type(m.root, "brand_color", "red");
    submit(m.root);
    await settle();
    expect((calls.find(c => c.method === "PATCH")!.body as { config: { brand_color: string } }).config.brand_color).toBe("red");
    expect(m.root.querySelector('[role="alert"]')!.textContent).toBe("The accent colour (brand_color) is #rrggbb.");
  });
});
