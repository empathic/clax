import { describe, expect, it } from "vitest";
import type { Artifact } from "../api";
import { filterArtifacts, orderArtifacts, publisherText, rally } from "./gallery-model";

const a = (title: string, description: string | null, extra: Partial<Artifact> = {}) => ({ id: title, title, description, icon: null, updated_at: "x", current_version: 1, pinned: false, ...extra }) as Artifact;

describe("gallery model", () => {
  it("matches title or description, case-insensitively, ignoring surrounding spaces", () => {
    const list = [a("Quarterly Review", null), a("Notes", "the REVIEW draft"), a("Other", null)];
    expect(filterArtifacts(list, "  review ").map(x => x.title)).toEqual(["Quarterly Review", "Notes"]);
    expect(filterArtifacts(list, "")).toBe(list);
  });
  it("names the owner's harness, else the command line", () => {
    expect(publisherText(a("x", null, { owner_session_id: "s", owner_harness: "codex" }))).toBe("published by codex");
    expect(publisherText(a("x", null, { owner_harness: null }))).toBe("published from the command line");
    expect(publisherText(a("x", null))).toBe("published from the command line");
  });
  it("puts pinned first, then the most recent", () => {
    const A = (id: string, pinned: boolean, at: string) => ({ id, title: id, description: null, icon: null, pinned, current_version: 1, updated_at: at });
    expect(orderArtifacts([A("old", false, "2026-09-01"), A("pin", true, "2026-08-01"), A("new", false, "2026-09-30")]).map(x => x.id)).toEqual(["pin", "new", "old"]);
  });
  it("marks the tenth version, and only it", () => {
    expect([9, 10, 11, 20].map(n => rally({ current_version: n } as never))).toEqual([false, true, false, false]);
  });
});
