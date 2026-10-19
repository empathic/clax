import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
  DEFAULTS, MAX_WIDTH, MIN_FLOAT_H, MIN_FLOAT_W, MIN_HEIGHT, MIN_STAGE_H, MIN_STAGE_W, MIN_WIDTH,
  DRAWER_KEY, applyPrefs, clampFloat, dockHeight, dockWidth, keyMove, moved, normal, readPrefs, reset, sizeOf,
} from "./drawer-model";
import { drawer } from "../ui/drawer.svelte";

afterEach(() => { localStorage.clear(); document.documentElement.removeAttribute("style"); delete document.documentElement.dataset.drawer; });
const view = { w: 1440, h: 840 };
const ui = (f: string) => readFileSync(join(__dirname, "..", "ui", f), "utf8");

describe("drawer prefs", () => {
  it("default to docked right at 392 px, with the floating window at its default place", () => {
    expect(readPrefs()).toEqual({});
    expect(normal(readPrefs())).toEqual({ layout: "right", w: 392, h: 280 });
  });

  it("are remembered, and what is missing or wrong reads as the default", () => {
    const p = { layout: "float" as const, w: 500, h: 300, x: 10, y: 20, fw: 400, fh: 500 };
    drawer.set(p);
    expect(JSON.parse(localStorage.getItem(DRAWER_KEY)!)).toEqual(p);
    expect(document.documentElement.dataset.drawer).toBe("float");
    expect(document.documentElement.style.getPropertyValue("--drawer-fw")).toBe("400px");
    // A drag's moves are shown, and remembered only where it ends.
    drawer.set({ ...p, w: 600 }, false);
    expect(drawer.prefs.w).toBe(600);
    expect(JSON.parse(localStorage.getItem(DRAWER_KEY)!)).toEqual(p);
    expect(normal(readPrefs())).toEqual(p);
    localStorage.setItem(DRAWER_KEY, "{not json");
    expect(readPrefs()).toEqual({});
    localStorage.setItem(DRAWER_KEY, "null");
    expect(readPrefs()).toEqual({});
    expect(normal({ layout: "top", w: "wide", h: null, x: 1 })).toEqual(DEFAULTS);
    expect(normal({ layout: "left", w: 9999, h: 3, x: -5, y: 2, fw: 10, fh: 10 }))
      .toEqual({ layout: "left", w: MAX_WIDTH, h: MIN_HEIGHT, x: 0, y: 2, fw: MIN_FLOAT_W, fh: MIN_FLOAT_H });
  });

  it("survive storage that throws", () => {
    const get = Storage.prototype.getItem;
    const set = Storage.prototype.setItem;
    Storage.prototype.getItem = () => { throw new Error("denied"); };
    Storage.prototype.setItem = () => { throw new Error("denied"); };
    try {
      expect(readPrefs()).toEqual({});
      expect(() => drawer.set(DEFAULTS)).not.toThrow();
      expect(drawer.prefs).toEqual(DEFAULTS);
    } finally {
      Storage.prototype.getItem = get;
      Storage.prototype.setItem = set;
    }
  });

  it("are laid out by the artifact page's head script before the first paint as applyPrefs lays them out", () => {
    const html = readFileSync(join(__dirname, "..", "..", "artifact.html"), "utf8");
    const head = /<script id="clax-drawer">([\s\S]*?)<\/script>/.exec(html)![1];
    const root = document.documentElement;
    const shown = () => [root.dataset.drawer ?? "", ...Array.from({ length: root.style.length }, (_, i) => root.style.item(i)).map(k => `${k}:${root.style.getPropertyValue(k)}`)];
    for (const stored of [null, "{not json", "null", '{"layout":"left","w":500}', '{"layout":"float","w":400,"h":300,"x":1,"y":2,"fw":300,"fh":400}', '{"w":"1px;color:red","h":null}']) {
      if (stored === null) localStorage.removeItem(DRAWER_KEY);
      else localStorage.setItem(DRAWER_KEY, stored);
      root.removeAttribute("style");
      delete root.dataset.drawer;
      new Function(head)();
      const fromHead = shown();
      root.removeAttribute("style");
      delete root.dataset.drawer;
      applyPrefs(readPrefs());
      expect(fromHead, stored ?? "nothing stored").toEqual(shown());
    }
  });

  it("lay the drawer out by the root's data-drawer and the sizes that are numbers", () => {
    const root = document.documentElement;
    const vars = () => Array.from({ length: root.style.length }, (_, i) => root.style.item(i)).map(k => `${k}:${root.style.getPropertyValue(k)}`);
    applyPrefs({ layout: "float", w: 400, h: 300, x: 1, y: 2, fw: 300, fh: 400 });
    expect(root.dataset.drawer).toBe("float");
    expect(vars()).toEqual(["--drawer-w:400px", "--drawer-h:300px", "--drawer-x:1px", "--drawer-y:2px", "--drawer-fw:300px", "--drawer-fh:400px"]);
    applyPrefs({ w: "1px;color:red" as never, h: Infinity });
    expect(root.dataset.drawer).toBe("");
    expect(vars()).toEqual([]);
  });
});

describe("drawer-model", () => {
  it("clamps a docked size to its limits and leaves the page its least size", () => {
    expect(dockWidth(100, view.w)).toBe(MIN_WIDTH);
    expect(dockWidth(5000, view.w)).toBe(MAX_WIDTH);
    expect(dockWidth(700, 900)).toBe(900 - MIN_STAGE_W);
    // A viewer too narrow for both keeps the drawer's least width.
    expect(dockWidth(400, 500)).toBe(MIN_WIDTH);
    expect(dockHeight(10, view.h)).toBe(MIN_HEIGHT);
    expect(dockHeight(5000, view.h)).toBe(view.h - MIN_STAGE_H);
  });

  it("keeps the floating window inside the viewer", () => {
    expect(clampFloat({ x: 1300, y: -40, w: 400, h: 500 }, view)).toEqual({ x: 1040, y: 0, w: 400, h: 500 });
    expect(clampFloat({ x: 0, y: 0, w: 4000, h: 4000 }, { w: 800, h: 600 })).toEqual({ x: 0, y: 0, w: 800, h: 600 });
    expect(clampFloat({ x: 100, y: 100, w: 10, h: 10 }, view)).toEqual({ x: 100, y: 100, w: MIN_FLOAT_W, h: MIN_FLOAT_H });
  });

  it("drags each docked edge the way it faces", () => {
    const box = { x: 0, y: 0, w: 392, h: 280 };
    expect(moved({ ...DEFAULTS, layout: "right" }, "dock", box, -100, 0, view).w).toBe(492);
    expect(moved({ ...DEFAULTS, layout: "left" }, "dock", box, -100, 0, view).w).toBe(292);
    expect(moved({ ...DEFAULTS, layout: "bottom" }, "dock", box, 0, -50, view).h).toBe(330);
    expect(moved({ ...DEFAULTS, layout: "right" }, "dock", box, 2000, 0, view).w).toBe(MIN_WIDTH);
  });

  it("moves and resizes the floating window within the viewer", () => {
    const p = { ...DEFAULTS, layout: "float" as const };
    const box = { x: 100, y: 100, w: 400, h: 500 };
    const f = (q: ReturnType<typeof moved>) => [q.x, q.y, q.fw, q.fh];
    expect(f(moved(p, "move", box, 50, 20, view))).toEqual([150, 120, 400, 500]);
    expect(f(moved(p, "move", box, 5000, -500, view))).toEqual([1040, 0, 400, 500]);
    expect(f(moved(p, "fw", box, 60, 60, view))).toEqual([100, 100, 460, 500]);
    expect(f(moved(p, "fh", box, 60, 60, view))).toEqual([100, 100, 400, 560]);
    // An edge stops at the viewer's edge rather than pushing the window back.
    expect(f(moved(p, "corner", box, 5000, 5000, view))).toEqual([100, 100, 1340, 740]);
    // A docked edge does nothing to the floating window, nor the window's grips to a docked drawer.
    expect(moved(p, "dock", box, 50, 50, view)).toEqual(p);
    expect(moved(DEFAULTS, "move", box, 50, 50, view)).toEqual(DEFAULTS);
  });

  it("resets the dragged size, and only it, to its default", () => {
    const p = { layout: "right" as const, w: 600, h: 400, x: 1, y: 2, fw: 300, fh: 300 };
    expect(reset(p, "dock")).toEqual({ ...p, w: 392 });
    expect(reset({ ...p, layout: "bottom" }, "dock")).toEqual({ ...p, layout: "bottom", h: 280 });
    expect(reset({ ...p, layout: "float" }, "corner")).toEqual({ layout: "float", w: 600, h: 400 });
  });

  it("moves an edge by arrow keys across it, further with Shift, and to its ends with Home and End", () => {
    expect(keyMove("ArrowLeft", false, "dock", "right")).toEqual([-16, 0]);
    expect(keyMove("ArrowRight", true, "dock", "left")).toEqual([64, 0]);
    expect(keyMove("ArrowUp", false, "dock", "right")).toBeNull();
    expect(keyMove("ArrowUp", false, "dock", "bottom")).toEqual([0, -16]);
    expect(keyMove("ArrowLeft", false, "dock", "bottom")).toBeNull();
    expect(keyMove("ArrowDown", false, "fh", "float")).toEqual([0, 16]);
    expect(keyMove("ArrowDown", false, "fw", "float")).toBeNull();
    expect(keyMove("ArrowUp", false, "move", "float")).toEqual([0, -16]);
    expect(keyMove("Enter", false, "dock", "right")).toBeNull();
    const box = { x: 0, y: 0, w: 392, h: 280 };
    const end = (layout: "right" | "left" | "bottom", key: string) => moved({ ...DEFAULTS, layout }, "dock", box, ...keyMove(key, false, "dock", layout)!, view);
    expect(end("right", "End").w).toBe(MAX_WIDTH);
    expect(end("left", "Home").w).toBe(MIN_WIDTH);
    expect(end("bottom", "End").h).toBe(view.h - MIN_STAGE_H);
    expect(end("bottom", "Home").h).toBe(MIN_HEIGHT);
  });

  it("announces each separator's size and range", () => {
    const box = { x: 1000, y: 100, w: 392, h: 500 };
    expect(sizeOf("dock", DEFAULTS, box, view)).toEqual({ now: 392, min: MIN_WIDTH, max: MAX_WIDTH });
    expect(sizeOf("dock", { ...DEFAULTS, layout: "bottom" }, box, view)).toEqual({ now: 500, min: MIN_HEIGHT, max: view.h - MIN_STAGE_H });
    expect(sizeOf("fw", { ...DEFAULTS, layout: "float" }, box, view)).toEqual({ now: 392, min: MIN_FLOAT_W, max: 440 });
    expect(sizeOf("move", { ...DEFAULTS, layout: "float" }, box, view)).toBeNull();
  });

  it("clamps with the limits the drawer's box clamps by", () => {
    expect(ui("SidebarIsland.svelte")).toContain(`width: min(var(--drawer-w, 392px), 100% - ${MIN_STAGE_W}px); min-width: ${MIN_WIDTH}px;`);
    expect(ui("SidebarIsland.svelte")).toContain(`height: min(var(--drawer-h, 280px), 100% - ${MIN_STAGE_H}px); min-height: ${MIN_HEIGHT}px;`);
    expect(ui("DrawerChrome.svelte")).toContain(`clamp(${MIN_FLOAT_W}px, var(--drawer-fw, 392px), 100%)`);
    expect(ui("DrawerChrome.svelte")).toContain(`clamp(${MIN_FLOAT_H}px, var(--drawer-fh, 100% - 32px), 100%)`);
  });
});
