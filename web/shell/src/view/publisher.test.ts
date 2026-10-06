import { describe, expect, it } from "vitest";
import type { Artifact } from "../api";
import { publisherText } from "./publisher";

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
