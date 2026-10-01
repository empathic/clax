import { describe, expect, it } from "vitest";
import { SKELETON_HTML, skeleton } from "./skeleton";

describe("skeleton", () => {
  it("creates the page once and finds the same elements again", () => {
    const root = document.createElement("div");
    const a = skeleton(root);
    const b = skeleton(root);
    expect(root.querySelectorAll(".page")).toHaveLength(1);
    expect(b.stage).toBe(a.stage);
    expect(a.title.textContent).toBe("Clax");
    expect(a.stage.parentElement).toBe(a.viewer);
    expect(a.stageIsland.parentElement).toBe(a.stage);
    expect(a.sidebarIsland.parentElement).toBe(a.viewer);
    expect(a.topbarIsland.parentElement?.classList.contains("topbar")).toBe(true);
  });
  it("adopts a page the server sent", () => {
    const root = document.createElement("div");
    root.innerHTML = `<div class="page">${SKELETON_HTML}</div>`;
    const page = root.firstElementChild;
    expect(skeleton(root).page).toBe(page);
  });
});
