import { describe, expect, it } from "vitest";
import { environment } from "./environment";
import { REDACTED } from "./scrub";

/** A window as `environment` reads it. */
function fakeWindow(over: Record<string, unknown> = {}) {
  return {
    location: { href: "https://app.example/orders/7?token=s3cret&tab=2#/reset?code=abc" },
    document: { title: "Orders for ahk@example.com" },
    innerWidth: 1440, innerHeight: 900, devicePixelRatio: 2, scrollX: 0, scrollY: 340.5,
    navigator: { userAgent: "Mozilla/5.0 (Macintosh)", platform: "MacIntel", language: "en-US", userAgentData: { platform: "macOS" } },
    matchMedia: (q: string) => ({ matches: q === "(prefers-color-scheme: dark)" }),
    ...over,
  } as unknown as Window;
}

describe("environment", () => {
  it("scrubs query values in url", () => {
    const env = environment(fakeWindow());
    expect(env.url).toBe(`https://app.example/orders/7?token=${REDACTED}&tab=${REDACTED}#/reset?code=${REDACTED}`);
    expect(env.title).toBe(`Orders for ${REDACTED}`);
  });

  it("reads the viewport, scroll and browser", () => {
    const env = environment(fakeWindow());
    expect(env).toMatchObject({
      viewport: { w: 1440, h: 900, dpr: 2 }, scroll: { x: 0, y: 340.5 },
      user_agent: "Mozilla/5.0 (Macintosh)", platform: "macOS", locale: "en-US", color_scheme: "dark",
    });
    expect(env.time_zone).toBe(Intl.DateTimeFormat().resolvedOptions().timeZone);
    expect(env.route).toBeUndefined();
  });

  it("falls back where the browser gives less", () => {
    const env = environment(fakeWindow({
      navigator: { platform: "Linux x86_64" }, matchMedia: undefined, devicePixelRatio: undefined, document: { title: "" },
    }));
    expect(env.platform).toBe("Linux x86_64");
    expect(env.viewport).toEqual({ w: 1440, h: 900, dpr: 1 });
    expect(env).not.toHaveProperty("color_scheme");
    expect(env).not.toHaveProperty("title");
    expect(env).not.toHaveProperty("user_agent");
    expect(environment(fakeWindow({ matchMedia: () => ({ matches: false }) })).color_scheme).toBe("light");
  });

  it("leaves out numbers JSON cannot carry and keeps strings well-formed", () => {
    const env = environment(fakeWindow({ innerWidth: Number.NaN, scrollY: Number.POSITIVE_INFINITY, document: { title: "half \ud83d" } }));
    expect(env).not.toHaveProperty("viewport");
    expect(env).not.toHaveProperty("scroll");
    expect(env.title).toBe("half �");
    expect(environment(fakeWindow({ devicePixelRatio: Number.NaN })).viewport).toEqual({ w: 1440, h: 900, dpr: 1 });
  });
});
