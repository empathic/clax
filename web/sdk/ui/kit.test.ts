// The UI kit (web/sdk/ui): its components render and behave (a button's
// states, a pin's place, a menu's keys, a dialog's focus and closing), and
// they are styled only
// through Clax's tokens: no colour literal in their styles, and every
// custom property they read is a role of web/sdk/tokens/clax.tokens.json
// (or the elevations and motion, which are not in it yet).
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { type Component, flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import { cssVariables, type TokenGroup } from "../src/tokens";
import { Button, Dialog, Menu, Pin, Quote, Surface, Thread } from "@empathic/clax-sdk/ui";

// Paths from __dirname: under jsdom, import.meta.url is no file: URL.
const dir = __dirname;
const file: TokenGroup = JSON.parse(readFileSync(join(dir, "../tokens/clax.tokens.json"), "utf8"));
const roles = new Set(["theme.light", "base"].flatMap(p => cssVariables(file, p)).map(l => /^--([\w-]+):/.exec(l)![1]));
const NOT_TOKENS_YET = new Set(["elev", "elev-lg", "t"]);

const styleOf = (name: string) => /<style>([\s\S]*?)<\/style>/.exec(readFileSync(join(dir, name), "utf8"))?.[1] ?? "";
const components = readdirSync(dir).filter(f => f.endsWith(".svelte"));

let mounted: Record<string, unknown>[] = [];
afterEach(() => { for (const m of mounted) void unmount(m); mounted = []; document.body.innerHTML = ""; });
// oxlint-disable-next-line typescript/no-explicit-any -- any component of the kit, with its props as the test gives them
const render = (c: Component<any>, props: Record<string, unknown>) => {
  const target = document.createElement("div");
  document.body.append(target);
  mounted.push(mount(c, { target, props }));
  flushSync();
  return target;
};

describe("the UI kit", () => {
  it.each(components)("styles %s only through Clax's tokens", name => {
    const css = styleOf(name);
    expect(css.length, name).toBeGreaterThan(0);
    expect(css, name).not.toMatch(/#[0-9a-f]{3,8}\b|\brgba?\(|\bhsla?\(/i);
    for (const [, v] of css.matchAll(/var\(--([\w-]+)\)/g)) expect(roles.has(v) || NOT_TOKENS_YET.has(v), `${name} reads --${v}`).toBe(true);
  });

  it("quotes text as text", () => {
    const el = render(Quote, { text: "<b>Total</b>" });
    expect(el.querySelector("p.quote")?.textContent).toBe("<b>Total</b>");
    expect(el.querySelector("b")).toBeNull();
  });

  it("shows a thread's comments in order, each ruled by its kind", () => {
    const el = render(Thread, { comments: [
      { id: "1", kind: "reporter", who: "You", body: "The total is wrong" },
      { id: "2", kind: "agent", who: "Triage agent", body: "Fixed, thanks" },
    ] });
    const cs = [...el.querySelectorAll(".c")];
    expect(cs.map(c => c.getAttribute("data-kind"))).toEqual(["reporter", "agent"]);
    expect(cs.map(c => c.querySelector(".who")?.textContent)).toEqual(["You", "Triage agent"]);
    expect(cs[1].querySelector(".body")?.textContent).toBe("Fixed, thanks");
  });

  it("gives a button its variant, its toggle state and its label, and calls it on a press", async () => {
    const { createRawSnippet } = await import("svelte");
    let pressed = 0;
    const el = render(Button, { variant: "primary", pill: true, pressed: true, label: "Report", onclick: () => { pressed++; }, children: createRawSnippet(() => ({ render: () => "<span>Report</span>" })) });
    const b = el.querySelector("button")!;
    expect([...b.classList].filter(c => !c.startsWith("svelte-"))).toEqual(["btn", "primary", "pill"]);
    expect(b.getAttribute("type")).toBe("button");
    expect(b.getAttribute("aria-pressed")).toBe("true");
    expect(b.getAttribute("aria-label")).toBe("Report");
    b.click();
    expect(pressed).toBe(1);
  });

  it("numbers a pin, names it, and places it where it is asked", () => {
    const el = render(Pin, { n: 3, label: "Thread 3", left: 12, top: 40, addressed: true });
    const p = el.querySelector("button")!;
    expect(p.textContent).toBe("3");
    expect(p.getAttribute("aria-label")).toBe("Thread 3");
    expect(p.style.left).toBe("12px");
    expect(p.style.top).toBe("40px");
    expect(p.classList.contains("addressed")).toBe(true);
  });

  it("opens a menu on its first item, moves with the arrows, wraps, chooses and closes", () => {
    const chosen: string[] = [];
    let closed = 0;
    const el = render(Menu, {
      label: "Thread actions",
      items: [{ id: "copy", label: "Copy link" }, { id: "off", label: "Off", disabled: true }, { id: "del", label: "Delete", danger: true }],
      onselect: (id: string) => chosen.push(id), onclose: () => { closed++; },
    });
    const menu = el.querySelector("[role=menu]")!;
    expect(menu.getAttribute("aria-label")).toBe("Thread actions");
    const items = [...el.querySelectorAll<HTMLButtonElement>("[role=menuitem]")];
    expect(document.activeElement).toBe(items[0]);
    const key = (k: string) => { document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true })); flushSync(); };
    key("ArrowDown");
    expect(document.activeElement).toBe(items[2]);
    key("ArrowDown");
    expect(document.activeElement).toBe(items[0]);
    key("ArrowUp");
    expect(document.activeElement).toBe(items[2]);
    expect(items[2].classList.contains("danger")).toBe(true);
    items[2].click();
    expect(chosen).toEqual(["del"]);
    key("Escape");
    expect(closed).toBe(1);
  });

  it("names a dialog by its title, focuses its first control, keeps Tab inside and closes on Escape or the scrim", async () => {
    const { createRawSnippet } = await import("svelte");
    let closed = 0;
    const el = render(Dialog, {
      title: "Delete this thread?", onclose: () => { closed++; },
      children: createRawSnippet(() => ({ render: () => "<p>It cannot be undone.</p>" })),
      actions: createRawSnippet(() => ({ render: () => "<span><button>Cancel</button><button>Delete</button></span>" })),
    });
    const dialog = el.querySelector("[role=dialog]")!;
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(document.getElementById(dialog.getAttribute("aria-labelledby")!)?.textContent).toBe("Delete this thread?");
    const [cancel, del] = [...el.querySelectorAll("button")];
    expect(document.activeElement).toBe(cancel);
    del.focus();
    del.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
    expect(document.activeElement).toBe(cancel);
    cancel.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(closed).toBe(1);
    (el.querySelector(".scrim") as HTMLElement).click();
    expect(closed).toBe(2);
    dialog.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(closed).toBe(2);
  });

  it("renders a surface as the element asked for, with its classes and label", async () => {
    const { createRawSnippet } = await import("svelte");
    const children = createRawSnippet(() => ({ render: () => "<span>inside</span>" }));
    const el = render(Surface, { as: "section", class: "panel", accent: true, label: "Your reports", children });
    const s = el.querySelector("section");
    // The caller's class first: it names what the surface is.
    expect([...(s?.classList ?? [])].filter(c => !c.startsWith("svelte-"))).toEqual(["panel", "surface", "accent"]);
    expect(s?.getAttribute("aria-label")).toBe("Your reports");
    expect(s?.textContent).toBe("inside");
  });
});
