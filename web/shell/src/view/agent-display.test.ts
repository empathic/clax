import { describe, expect, it } from "vitest";
import { withPeople } from "./agent-display";

const ALEX = { public_id: "u_alex", name: "Alex", owner: true };
const SAM = { public_id: "u_sam", name: "Sam", owner: false };
const agents = [
  { handle: "a_cx", harness: "codex", live: true, name: "codex", person: ALEX },
  { handle: "a_sam", harness: "claude", live: true, name: "sam-notes", person: SAM },
];
const names = new Map([["a_cx", "codex"], ["a_sam", "sam-notes"]]);

describe("withPeople", () => {
  it("names another person's agents with their person where two people's agents appear", () => {
    expect([...withPeople(names, agents, "u_alex")]).toEqual([["a_cx", "codex"], ["a_sam", "sam-notes (Sam's)"]]);
    expect([...withPeople(names, agents, "u_sam")]).toEqual([["a_cx", "codex (Alex's)"], ["a_sam", "sam-notes"]]);
  });
  it("leaves one person's agents by name alone", () => {
    expect([...withPeople(names, [agents[0]], "u_lan")]).toEqual([...names]);
  });
});
