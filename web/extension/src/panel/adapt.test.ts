import { describe, expect, it } from "vitest";
import { asPages, pageOfRoute } from "./adapt";

describe("adapt", () => {
  it("lets the sidebar read a thread's route as its page", () => {
    const t = (route?: string) => ({ id: "x", anchor: { kind: "element", selector: "body", file: "index.html", ...(route ? { route } : {}) } }) as never;
    const [a, b] = asPages([t(), t("?tab=billing")]);
    expect(a.anchor.file).toBe("index.html");
    expect(b.anchor.file).toBe("?tab=billing");
    expect(pageOfRoute(null)).toBe("index.html");
    expect(pageOfRoute("#/users/7")).toBe("#/users/7");
  });

  it("keeps a thread's clip URL, which says the worker has a clip to fetch for the panel", () => {
    const t = { id: "x", has_clip: true, clip_url: "/api/artifacts/a/threads/x/clip", anchor: { kind: "element", selector: "body", file: "index.html" } } as never;
    expect(asPages([t])[0].clip_url).toBe("/api/artifacts/a/threads/x/clip");
  });
});
