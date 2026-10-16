import { describe, expect, it } from "vitest";
import { Refused } from "./api";
import { peopleDenied, refusal } from "./refusal";

describe("refusal", () => {
  it("words the refusals a person can act on, and makes any other a sentence", () => {
    expect(refusal(new Refused(409, "the name 'Аlex' is taken", "name_taken"))).toBe("That name is taken, or reads too like a name already here: choose another.");
    expect(refusal(new Refused(400, "that is not the request's code", "wrong_code"))).toBe("That is not the code the machine shows. Check it on that machine and type it again.");
    expect(refusal(new Refused(400, "that is not the request's code; the request is now denied", "wrong_code"))).toBe("That is not the code the machine shows, a third time: the request is denied.");
    expect(refusal(new Refused(409, "the request is no longer pending", "conflict"))).toBe("The request is no longer pending.");
    expect(refusal(new Error("Already done."))).toBe("Already done.");
  });
  it("says why People is not this browser's: never connected, or disconnected", () => {
    const e = new Refused(403, "only the owner or a connected person has People", "forbidden");
    expect(peopleDenied(e, false)).toMatch(/^People is for the owner of this Clax/);
    expect(peopleDenied(e, true)).toMatch(/^You are no longer connected/);
  });
});
