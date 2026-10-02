import { describe, expect, it } from "vitest";
import type { Artifact } from "../api";
import { filterArtifacts, publisherText } from "./gallery-model";

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
});
