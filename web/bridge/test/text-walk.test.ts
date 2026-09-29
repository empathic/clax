import { describe, expect, it } from "vitest";
import { charsBetween, pointInto } from "../src/text-walk";

describe("charsBetween and pointInto", () => {
  it("count the characters a reader sees, across elements and past scripts", () => {
    document.body.innerHTML = `<pre id="p">ab<span>cd</span><script>xx</script>e\nfg</pre>`;
    const pre = document.getElementById("p")!;
    const [ab, cd, rest] = [pre.firstChild as Text, pre.querySelector("span")!.firstChild as Text, pre.lastChild as Text];
    expect(charsBetween(pre, { node: ab, offset: 1 }, { node: cd, offset: 1 })).toBe(2);
    expect(charsBetween(pre, { node: ab, offset: 0 }, { node: rest, offset: 3 })).toBe(7);
    expect(charsBetween(pre, { node: cd, offset: 0 }, { node: cd, offset: 2 })).toBe(2);
    expect(pointInto(pre, 3)).toEqual({ node: cd, offset: 1 });
    expect(pointInto(pre, 5)).toEqual({ node: rest, offset: 1 });
    expect(pointInto(pre, 99)).toBeNull();
  });
});
