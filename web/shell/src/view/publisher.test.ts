import { describe, expect, it } from "vitest";
import type { Artifact } from "../api";
import { publisherText, setBy } from "./publisher";

const a = (title: string, description: string | null, extra: Partial<Artifact> = {}) => ({ id: title, title, description, icon: null, updated_at: "x", current_version: 1, pinned: false, ...extra }) as Artifact;

describe("publisherText", () => {
  it("names the owner's harness, else the command line", () => {
    expect(publisherText(a("x", null, { owner_session_id: "s", owner_harness: "codex" }))).toBe("published by codex");
    expect(publisherText(a("x", null, { owner_harness: null }))).toBe("published from the command line");
    expect(publisherText(a("x", null))).toBe("published from the command line");
  });
  it("names a live page by its URL", () => {
    expect(publisherText(a("x", null, { kind: "live", live: { origin: "http://localhost:5173", path: "/settings", page_url: "http://localhost:5173/settings" } }))).toBe("Live page · localhost:5173/settings");
  });
});

describe("setBy", () => {
  const fork = (title: string | null, extra: Partial<Artifact> = {}) => a("Fork", null, { forked_from: { artifact_id: "7q3k9mzx2b4t", version: 2, title }, ...extra });
  it("links a fork to the version it came from, after its publisher", () => {
    const el = document.createElement("span");
    setBy(el, fork("Plan", { owner_harness: "claude" }));
    expect(el.textContent).toBe("published by claude · Forked from Plan v2");
    const link = el.querySelector<HTMLAnchorElement>("a.lineage")!;
    expect(link.getAttribute("href")).toBe("/a/7q3k9mzx2b4t/v/2");
  });
  it("says only where a browser's fork came from, and links no source the viewer may not read", () => {
    const el = document.createElement("span");
    setBy(el, fork("Plan"));
    expect(el.textContent).toBe("Forked from Plan v2");
    setBy(el, fork(null));
    expect(el.textContent).toBe("Forked from an artifact you cannot see");
    expect(el.querySelector("a")).toBeNull();
    expect(el.querySelector(".lineage")).not.toBeNull();
  });
  it("leaves the line alone when nothing changed, and is plain text for no fork", () => {
    const el = document.createElement("span");
    setBy(el, fork("Plan"));
    const link = el.querySelector("a");
    setBy(el, fork("Plan"));
    expect(el.querySelector("a")).toBe(link);
    setBy(el, a("x", null, { owner_harness: "codex" }));
    expect(el.innerHTML).toBe("published by codex");
    setBy(el, null);
    expect(el.textContent).toBe("");
  });
});
