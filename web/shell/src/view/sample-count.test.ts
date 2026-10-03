import { expect, it } from "vitest";
import { callCountText } from "./sample-count";

it("counts calls today, against the cap when there is one", () => {
  expect(callCountText(0, null)).toBe("0 Claude calls today");
  expect(callCountText(1, null)).toBe("1 Claude call today");
  expect(callCountText(3, 200)).toBe("3 of 200 Claude calls today");
});
