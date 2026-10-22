import { describe, expect, it } from "vitest";
import { DESC_TEXT, NAME_TEXT, describeElement, nameOf } from "../src/describe";

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

describe("nameOf", () => {
  it("is the element's accessible name as the page gives it", () => {
    expect(nameOf(el(`<button aria-label="Save">💾</button>`))).toBe("Save");
    expect(nameOf(el(`<img alt="A cat">`))).toBe("A cat");
    expect(nameOf(el(`<span title="Due date"></span>`))).toBe("Due date");
    document.body.innerHTML = `<h3 id="t">Weekly  plan</h3><section aria-labelledby="t x"><p>Mon</p></section>`;
    expect(nameOf(document.querySelector("section")!)).toBe("Weekly plan");
    expect(nameOf(el(`<table><caption>Runs by week</caption><tr><td>1</td></tr></table>`))).toBe("Runs by week");
    expect(nameOf(el(`<figure><img src="x.png"><figcaption>Route map</figcaption></figure>`))).toBe("Route map");
  });

  it("is else the first characters of the text a reader sees in it", () => {
    expect(nameOf(el(`<div><b>Mon</b><span>5 km</span></div>`))).toBe("Mon 5 km");
    const text = "abcdefghij".repeat(8);
    expect(NAME_TEXT).toBe(60);
    expect(nameOf(el(`<div>${text}</div>`))).toBe(text.slice(0, 60));
  });

  it("is else what it is, under the nearest heading before it", () => {
    document.body.innerHTML = `<h1>Runs</h1><section><h2>p95 by four-hour window</h2><div><canvas></canvas></div></section>`;
    expect(nameOf(document.querySelector("canvas")!)).toBe("Chart under ‘p95 by four-hour window’");
    document.body.innerHTML = `<h2>Photos</h2><p>x</p><img src="a.png"><div role="heading">Later</div>`;
    expect(nameOf(document.querySelector("img")!)).toBe("Image under ‘Photos’");
    document.body.innerHTML = `<table><tr><td></td></tr></table>`;
    expect(nameOf(document.querySelector("table")!)).toBe("Table");
    document.body.innerHTML = `<svg role="img"></svg><svg></svg>`;
    expect(nameOf(document.querySelectorAll("svg")[0])).toBe("Image");
    expect(nameOf(document.querySelectorAll("svg")[1])).toBe("Chart");
  });

  it("is null for an element with no name, no text and no kind word", () => {
    expect(nameOf(el(`<div aria-label="  "><span></span></div>`))).toBeNull();
    expect(nameOf(el(`<div aria-label="\u0007"></div>`))).toBeNull();
  });

  it("never holds a line break or control character, and is at most 120 characters", () => {
    expect(nameOf(el(`<button aria-label="Save\nnow\u0007">x</button>`))).toBe("Save now");
    document.body.innerHTML = `<h2>${"h".repeat(200)}</h2><canvas></canvas>`;
    expect([...nameOf(document.querySelector("canvas")!)!].length).toBeLessThanOrEqual(120);
  });
});
