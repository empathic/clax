// The UI kit (web/sdk/ui): its components render, and they are styled only
// through Clax's tokens: no colour literal in their styles, and every
// custom property they read is a role of web/sdk/tokens/clax.tokens.json
// (or the elevations and motion, which are not in it yet).
import { readdirSync, readFileSync } from "node:fs";
import { type Component, flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import { cssVariables, type TokenGroup } from "../src/tokens";
import { Quote, Surface, Thread } from "@empathic/clax-sdk/ui";

const dir = new URL(".", import.meta.url);
const file: TokenGroup = JSON.parse(readFileSync(new URL("../tokens/clax.tokens.json", import.meta.url), "utf8"));
const roles = new Set(["theme.light", "base"].flatMap(p => cssVariables(file, p)).map(l => /^--([\w-]+):/.exec(l)![1]));
const NOT_TOKENS_YET = new Set(["elev", "elev-lg", "t"]);

const styleOf = (name: string) => /<style>([\s\S]*?)<\/style>/.exec(readFileSync(new URL(name, dir), "utf8"))?.[1] ?? "";
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

  it("renders a surface as the element asked for, with its classes and attributes", async () => {
    const { createRawSnippet } = await import("svelte");
    const children = createRawSnippet(() => ({ render: () => "<span>inside</span>" }));
    const el = render(Surface, { as: "section", class: "panel", accent: true, "aria-label": "Your reports", children });
    const s = el.querySelector("section");
    expect(s?.classList.contains("surface")).toBe(true);
    expect(s?.classList.contains("accent")).toBe(true);
    expect(s?.classList.contains("panel")).toBe(true);
    expect(s?.getAttribute("aria-label")).toBe("Your reports");
    expect(s?.textContent).toBe("inside");
  });
});
