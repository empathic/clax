import { describe, expect, it } from "vitest";
import { linkedPage } from "../src/nav";

const SUB = "http://7q3k9mzx2b4t.localhost:7480/v/2/";
const MAIN = "http://localhost:7480/c/7q3k9mzx2b4t/v/2/";

describe("linkedPage", () => {
  it("names the version's page a link leads to, from any page, in both frame modes", () => {
    expect(linkedPage("about.html", SUB, "index.html")).toBe("about.html");
    expect(linkedPage("about.html", MAIN, "index.html")).toBe("about.html");
    expect(linkedPage("index.html", `${SUB}about.html`, "about.html")).toBe("index.html");
    expect(linkedPage("./", `${MAIN}about.html`, "about.html")).toBe("index.html");
    expect(linkedPage("../a%20b.html", `${SUB}docs/x.html`, "docs/x.html")).toBe("a b.html");
    expect(linkedPage("docs/x.html", MAIN, "index.html")).toBe("docs/x.html");
  });

  it("leaves other links alone", () => {
    expect(linkedPage("#top", SUB, "index.html"), "same page").toBeNull();
    expect(linkedPage("", `${SUB}about.html`, "about.html"), "same page").toBeNull();
    expect(linkedPage("about.html?x=1", SUB, "index.html"), "a query the shell URL cannot carry").toBeNull();
    expect(linkedPage("https://example.com/", SUB, "index.html"), "another site").toBeNull();
    expect(linkedPage("/v/1/about.html", SUB, "index.html"), "another version").toBeNull();
    expect(linkedPage("/c/9zzzzzzzzzzz/v/2/a.html", MAIN, "index.html"), "another artifact").toBeNull();
    expect(linkedPage("mailto:x@y.z", SUB, "index.html")).toBeNull();
  });
});
