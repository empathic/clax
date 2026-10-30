import { beforeEach, describe, expect, it } from "vitest";
import { KeyboardPick } from "./keyboard-pick";

let outlined: (string | null)[];
let picked: string[];
let kb: KeyboardPick;
const press = (key: string, target: EventTarget = document.body) => {
  const e = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, composed: true });
  target.dispatchEvent(e);
  return e;
};
const at = () => outlined.at(-1);

beforeEach(() => {
  document.body.innerHTML = `<main id="m"><h2 id="h">Quarterly goals</h2><section id="s"><p id="p1">Grow revenue</p><p id="p2">Hire</p></section></main><input id="f"><clax-overlay id="o"></clax-overlay>`;
  outlined = [];
  picked = [];
  kb?.set(false);
  kb = new KeyboardPick(document, { outline: t => outlined.push(t?.id ?? null), pick: el => picked.push(el.id), hover: () => {} });
});

describe("keyboard picking", () => {
  it("outlines with the arrow keys and picks with Enter, only once turned on", () => {
    expect(press("ArrowDown").defaultPrevented).toBe(false);
    kb.set(true);
    expect(press("ArrowDown").defaultPrevented).toBe(true);
    // The first press outlines the element at the centre, else the first (jsdom has none at the centre).
    expect(at()).toBe("m");
    press("ArrowRight");
    expect(at()).toBe("h");
    press("ArrowDown");
    expect(at()).toBe("s");
    press("ArrowDown");
    expect(at()).toBe("f");
    // The overlay is never outlined: Down stays.
    press("ArrowDown");
    expect(at()).toBe("f");
    press("ArrowUp");
    expect(at()).toBe("s");
    press("ArrowUp");
    expect(at()).toBe("h");
    press("ArrowDown");
    press("ArrowRight");
    press("ArrowDown");
    expect(at()).toBe("p2");
    press("ArrowLeft");
    expect(at()).toBe("s");
    expect(press("Enter").defaultPrevented).toBe(true);
    expect(picked).toEqual(["s"]);
    expect(at()).toBeNull();
    // Nothing outlined, or a pick still being taken: Enter is the page's.
    expect(press("Enter").defaultPrevented).toBe(false);
    press("ArrowDown");
    expect(press("Enter").defaultPrevented).toBe(false);
    kb.done();
    expect(press("Enter").defaultPrevented).toBe(true);
    expect(picked).toEqual(["s", "m"]);
  });

  it("leaves keys typed into a field, and those with a modifier, to the page", () => {
    kb.set(true);
    expect(press("ArrowDown", document.getElementById("f")!).defaultPrevented).toBe(false);
    const alt = new KeyboardEvent("keydown", { key: "ArrowDown", altKey: true, bubbles: true, cancelable: true });
    document.body.dispatchEvent(alt);
    expect(alt.defaultPrevented).toBe(false);
    expect(outlined).toEqual([]);
    kb.set(false);
    expect(press("ArrowDown").defaultPrevented).toBe(false);
  });
});

describe("keys pressed in the overlay", () => {
  /** The overlay host's shadow root: a mode bar button, a button and a drawing stage. */
  function overlayHost() {
    const host = document.getElementById("o")!;
    const root = host.shadowRoot ?? host.attachShadow({ mode: "open" });
    root.innerHTML = `<div role="toolbar"><button id="mode">Element</button></div><button id="send">Send</button><div id="stage" tabindex="0"></div>`;
    return { root, mode: root.getElementById("mode")!, send: root.getElementById("send")!, stage: root.getElementById("stage")! };
  }

  it("are the overlay's own: Enter on its button while an element is outlined, and arrows on its stage", () => {
    const o = overlayHost();
    kb.set(true);
    press("ArrowDown");
    expect(at()).toBe("m");
    const enter = press("Enter", o.send);
    expect(enter.defaultPrevented).toBe(false);
    expect(picked).toEqual([]);
    expect(press(" ", o.send).defaultPrevented).toBe(false);
    expect(press("ArrowDown", o.stage).defaultPrevented).toBe(false);
    expect(press("Enter", o.stage).defaultPrevented).toBe(false);
    expect(at()).toBe("m");
    expect(picked).toEqual([]);
  });

  it("the mode bar's arrows stay the bar's; start() outlines the element in the middle, and Enter picks it", () => {
    const o = overlayHost();
    kb.set(true);
    o.mode.focus();
    expect(o.root.activeElement).toBe(o.mode);
    expect(press("ArrowDown", o.mode).defaultPrevented).toBe(false);
    expect(press("ArrowRight", o.mode).defaultPrevented).toBe(false);
    expect(outlined).toEqual([]);
    expect(o.root.activeElement).toBe(o.mode);
    expect(kb.start()).toBe(true);
    expect(at()).toBe("m");
    expect(press("Enter").defaultPrevented).toBe(true);
    expect(picked).toEqual(["m"]);
    // Off, it starts nothing.
    kb.set(false);
    expect(kb.start()).toBe(false);
  });

  it("goes on from what a pointer outlined", () => {
    kb.set(true);
    kb.follow(document.getElementById("p1"));
    press("ArrowDown");
    expect(at()).toBe("p2");
  });
});
