import assert from "node:assert/strict";
import { test } from "node:test";
import { daysBetween } from "./days.js";

test("Monday to Friday is 5 days of leave, both ends included", () => {
  assert.equal(daysBetween("2026-03-02", "2026-03-06"), 5);
});

test("a leave of one single day is 1 day", () => {
  assert.equal(daysBetween("2026-03-02", "2026-03-02"), 1);
});
