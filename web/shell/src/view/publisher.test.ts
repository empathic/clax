import { describe, expect, it } from "vitest";
import type { Artifact, PersonCard } from "../api";
import { appByline, forksText, publisherText, setBy } from "./publisher";

const a = (title: string, description: string | null, extra: Partial<Artifact> = {}) => ({ id: title, title, description, icon: null, updated_at: "x", current_version: 1, pinned: false, ...extra }) as Artifact;

describe("publisherText", () => {
  it("names the owner's harness, else the command line", () => {
    expect(publisherText(a("x", null, { owner_session_id: "s", owner_harness: "codex" }))).toBe("published by codex");
    expect(publisherText(a("x", null, { owner_harness: null }))).toBe("published from the command line");
    expect(publisherText(a("x", null))).toBe("published from the command line");
  });
  it("names the owner's agent by its name where it has one (spec 2026-10-08 §11)", () => {
    expect(publisherText(a("x", null, { owner_harness: "claude", owner_name: "web" }))).toBe("published by web");
  });
  it("names who started an artifact from a brief, until an agent publishes it", () => {
    expect(publisherText(a("x", null, { brief: { by: "Rae" } }))).toBe("started from a brief by Rae");
    expect(publisherText(a("x", null, { brief: { by: "Rae" }, owner_harness: "claude", owner_name: "web" }))).toBe("published by web");
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

describe("the app header", () => {
  // Artifact v8 carries release 3 of the app's bundle.
  const app = (by: PersonCard, owner: PersonCard = { name: "Alex", login: "akesling" }, bundle_version = 3) => ({ version: 8, bundle_version, installs: 12, published_by: by, owner });

  it("appByline numbers the bundle's release and names editor and owner", () => {
    expect(appByline(app({ name: "Jun", login: "junp" }))).toBe("v3 by @junp · by @akesling");
    expect(appByline(app({ name: "Alex", login: "akesling" }))).toBe("v3 · by @akesling");
    // A person with no login goes by their name.
    expect(appByline(app({ name: "Sam", login: null }, { name: "Ada", login: null }, 2))).toBe("v2 by Sam · by Ada");
    expect(appByline(app({ name: "Ada", login: null }, { name: "Ada", login: null }, 2))).toBe("v2 · by Ada");
  });

  it("tells apart two people of one name by their public IDs, as the Get in Clax page does", () => {
    const sam = (public_id: string) => ({ name: "Sam", login: null, public_id });
    expect(appByline(app(sam("p_editor"), sam("p_owner")))).toBe("v3 by Sam · by Sam");
    expect(appByline(app(sam("p_owner"), sam("p_owner")))).toBe("v3 · by Sam");
  });

  it("counts forks", () => {
    expect(forksText(1)).toBe("1 fork");
    expect(forksText(4)).toBe("4 forks");
  });

  it("puts the app's byline, its installs and its forks on the line under the title", () => {
    const el = document.createElement("span");
    setBy(el, a("Habits", null, { current_version: 8, owner_harness: "claude", forks: 2, app: app({ name: "Jun", login: "junp" }) }));
    expect(el.textContent).toBe("v3 by @junp · by @akesling · 12 installs · 2 forks");
    setBy(el, a("Habits", null, { current_version: 8, forks: 0, app: { ...app({ name: "Alex", login: "akesling" }), installs: 1 } }));
    expect(el.textContent).toBe("v3 · by @akesling · 1 install");
    // No installs yet: no count, as for forks.
    setBy(el, a("Habits", null, { current_version: 1, app: { ...app({ name: "Alex", login: "akesling" }, undefined, 1), installs: 0 } }));
    expect(el.textContent).toBe("v1 · by @akesling");
    expect(el.classList.contains("app")).toBe(true);
    // An artifact that is no app keeps its publisher, with its forks.
    setBy(el, a("Plan", null, { owner_harness: "claude", forks: 1, app: null }));
    expect(el.textContent).toBe("published by claude · 1 fork");
    expect(el.classList.contains("app")).toBe(false);
  });
});
