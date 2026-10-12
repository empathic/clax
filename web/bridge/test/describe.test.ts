import { describe, expect, it } from "vitest";
import { describeElement } from "../src/describe";

const el = (html: string) => {
  document.body.innerHTML = html;
  return document.body.firstElementChild!;
};

describe("describeElement", () => {
  it("gives the tag, the role and the accessible name as the page gives them", () => {
    expect(describeElement(el(`<div role="button" aria-label="Save" title="t">x</div>`))).toEqual({ tag: "div", role: "button", name: "Save" });
    expect(describeElement(el(`<img alt="A cat">`))).toEqual({ tag: "img", role: null, name: "A cat" });
    expect(describeElement(el(`<span title="Due date"></span>`))).toEqual({ tag: "span", role: null, name: "Due date" });
    expect(describeElement(el(`<h2>Goals</h2>`))).toEqual({ tag: "h2", role: null, name: null });
  });
});
