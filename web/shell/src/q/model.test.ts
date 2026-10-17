import { describe, expect, it } from "vitest";
import { view } from "./fixtures";
import { agentLabel, answerLine, closedLabel, complete, cutHeader, emptyDraft, nextOpen, pick, previewOf, shownLabel, toBody, typeOther } from "./model";

describe("question model", () => {
  it("tracks completeness per kind and builds the body", () => {
    const q = view();
    let d = emptyDraft(q);
    expect(complete(q, d)).toEqual([false, false, false]);
    d = pick(q, 0, "Two", d);
    d = pick(q, 1, "Left", d); d = pick(q, 1, "Right", d); d = pick(q, 1, "Left", d);
    d = typeOther(q, 2, "  ship it ", d);
    expect(complete(q, d)).toEqual([true, true, true]);
    expect(toBody(q, d)).toEqual({ answers: [{ selected: ["Two"], text: null }, { selected: ["Right"], text: null }, { selected: [], text: "ship it" }] });
  });
  it("Other replaces a single choice and a pick clears Other", () => {
    const q = view();
    let d = typeOther(q, 0, "Three", pick(q, 0, "Two", emptyDraft(q)));
    expect(d[0]).toEqual({ selected: [], text: "Three" });
    d = pick(q, 0, "One", d);
    expect(d[0]).toEqual({ selected: ["One"], text: "" });
  });
  it("chooses the preview to show", () => {
    const s = view().questions[0];
    expect(previewOf(s, null, { selected: [], text: "" })).toBe("|a|b|");
    expect(previewOf(s, null, { selected: ["One"], text: "" })).toBe("|ab|");
    expect(previewOf(s, "Two", { selected: ["One"], text: "" })).toBe("|a|b|");
    expect(previewOf(view().questions[1], null, { selected: [], text: "" })).toBeNull();
  });
  it("finds the next unanswered question, wrapping round", () => {
    expect(nextOpen([false, false, false], -1)).toBe(0);
    expect(nextOpen([true, false, false], 0)).toBe(1);
    expect(nextOpen([false, true, true], 1)).toBe(0);
    expect(nextOpen([true, true, false], 2)).toBe(2);
    expect(nextOpen([true, true, true], 0)).toBe(-1);
  });
  it("reads an answer as the person gave it", () => {
    const [single, multi, free] = view().questions;
    expect(answerLine(single, { selected: ["Two"], text: null })).toBe("Two");
    expect(answerLine(single, { selected: [], text: "Three" })).toBe("Other: Three");
    expect(answerLine(multi, { selected: ["Left", "Right"], text: "" })).toBe("Left, Right");
    expect(answerLine(free, { selected: [], text: "ok" })).toBe("ok");
    expect(answerLine(free, undefined)).toBe("");
  });
  it("names agents and closed states", () => {
    const a = view(); const b = view({ id: "x", agent: { handle: "a_9c2b00", harness: "claude", project: "p" } });
    expect(agentLabel(a.agent, [a.agent])).toBe("claude");
    expect(agentLabel(a.agent, [a.agent, b.agent])).toBe("claude 1f3a");
    // An agent with a name goes by it (spec 2026-10-08 §11).
    expect(agentLabel({ ...a.agent!, name: "clax-multi-person" }, [a.agent, b.agent])).toBe("clax-multi-person");
    expect(closedLabel(view({ status: "released" }))).toBe("Moved to the terminal");
    expect(closedLabel(view({ status: "withdrawn" }))).toBe("claude stopped waiting");
    expect(closedLabel(view({ status: "answered", answered_via: "terminal" }))).toBe("Answered in the terminal");
    expect(cutHeader("A very long header")).toBe("A very long…");
  });

  it("keeps a multi choice's Other text beside its picks, in option order, and leaves blank text out", () => {
    const q = view({ questions: [{ ...view().questions[1], other: true }] });
    let d = pick(q, 0, "Right", emptyDraft(q));
    d = pick(q, 0, "Left", d);
    expect(d[0].selected).toEqual(["Left", "Right"]);
    d = typeOther(q, 0, " also top ", d);
    expect(toBody(q, d)).toEqual({ answers: [{ selected: ["Left", "Right"], text: "also top" }] });
    // Text alone answers a choice question that offers Other; blanks do not.
    expect(complete(q, typeOther(q, 0, "x", emptyDraft(q)))).toEqual([true]);
    expect(complete(q, typeOther(q, 0, "   ", emptyDraft(q)))).toEqual([false]);
    expect(toBody(q, pick(q, 0, "Left", emptyDraft(q))).answers[0].text).toBeNull();
    // A label that is not an option changes nothing.
    expect(pick(q, 0, "Nope", d)).toBe(d);
  });

  it("says when an option has no preview of its own, names a missing agent, and keeps short headers", () => {
    const s = { ...view().questions[0], options: [{ label: "A", preview: "p" }, { label: "B" }] };
    expect(previewOf(s, "B", { selected: [], text: "" })).toBe("");
    expect(agentLabel(null, [])).toBe("an agent");
    // Another Clax's agent: with whose Clax it asks from.
    expect(agentLabel({ handle: "a_rel", harness: "claude", project: null, name: "claude", via: "Alex's Clax" }, [])).toBe("claude · via Alex's Clax");
    expect(closedLabel(view({ status: "withdrawn", agent: null }))).toBe("an agent stopped waiting");
    expect(closedLabel(view({ status: "declined" }))).toBe("Skipped");
    expect(closedLabel(view({ status: "answered", answered_via: "shell" }))).toBe("Answered");
    expect(cutHeader("Layout")).toBe("Layout");
    expect(cutHeader("Twelve chars")).toBe("Twelve chars");
  });

  it("shows a recommended label without its trailing (Recommended)", () => {
    expect(shownLabel({ label: "Vitest (Recommended)", recommended: true })).toBe("Vitest");
    expect(shownLabel({ label: "Vitest  (recommended) ", recommended: true })).toBe("Vitest");
    expect(shownLabel({ label: "(Recommended)", recommended: true })).toBe("(Recommended)");
    expect(shownLabel({ label: "Jest (Recommended)" })).toBe("Jest (Recommended)");
    expect(shownLabel({ label: "Two", recommended: true })).toBe("Two");
  });
});
