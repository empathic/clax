import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const putPresence = vi.fn(async () => null);
vi.mock("../api", () => ({ getPresence: vi.fn(async () => null), putPresence: (...a: unknown[]) => putPresence(...(a as [])) }));

import { visibility } from "../stream";
import type { ViewState } from "./artifact-controller";
import { PresenceReporter } from "./presence-reporter";

const state = { shareWhere: false, threads: [], selected: null } as unknown as ViewState;

describe("PresenceReporter visibility", () => {
  const rule = visibility.hidden;
  let r: PresenceReporter | null = null;
  beforeEach(() => putPresence.mockClear());
  afterEach(() => {
    visibility.hidden = rule;
    r?.dispose();
    r = null;
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "visible" });
  });

  it("follows the shared hidden rule, not the document's own visibility (app mode)", () => {
    // The app's WebView reports its document hidden while the app shows it.
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
    visibility.hidden = () => false;
    r = new PresenceReporter("a1", () => state, () => true, () => {});
    r.report(true);
    expect(putPresence).toHaveBeenLastCalledWith("a1", "here", null, expect.any(String));
  });

  it("reports away when the rule says hidden, whatever the document says", () => {
    visibility.hidden = () => true;
    r = new PresenceReporter("a1", () => state, () => true, () => {});
    r.report(true);
    expect(putPresence).toHaveBeenLastCalledWith("a1", "away", null, expect.any(String));
  });
});
