import { describe, expect, it } from "vitest";
import { DESC_TEXT, describeElement } from "../src/describe";

const el = (html: string) => {
  document.body.innerHTML = html;
  return document.body.firstElementChild!;
};

describe("describeElement", () => {
  it("gives the tag, the role and the accessible name as the page gives them", () => {
    expect(describeElement(el(`<div role="button" aria-label="Save" title="t">x</div>`))).toEqual({ tag: "div", role: "button", name: "Save", text: "x" });
    expect(describeElement(el(`<img alt="A cat">`))).toEqual({ tag: "img", role: null, name: "A cat", text: null });
    expect(describeElement(el(`<span title="Due date"></span>`))).toEqual({ tag: "span", role: null, name: "Due date", text: null });
    expect(describeElement(el(`<h2>Goals</h2>`))).toEqual({ tag: "h2", role: null, name: null, text: "Goals" });
  });

  it("gives the text a reader sees, with a space where text runs from one element into another", () => {
    expect(describeElement(el(`<div><b>Mon</b><span>5 km</span></div>`)).text).toBe("Mon 5 km");
    expect(describeElement(el(`<p>Run <b>far</b>, then <i>rest</i>.</p>`)).text).toBe("Run far, then rest.");
    expect(describeElement(el(`<ul>\n  <li>One</li>\n  <li>Two<script>x()</script></li></ul>`)).text).toBe("One Two");
    expect(describeElement(el(`<p>${"word ".repeat(100)}</p>`)).text).toHaveLength(DESC_TEXT);
  });
});
