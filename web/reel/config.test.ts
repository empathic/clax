import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { checkCuts, parseReel, totalSeconds } from "./config.ts";

const here = dirname(fileURLToPath(import.meta.url));
const committed = readFileSync(join(here, "reel.toml"), "utf8");
const scripted = readdirSync(join(here, "vignettes")).filter(f => f.endsWith(".ts")).map(f => f.slice(0, -3));

/** A reel.toml with `vignettes` (TOML tables) in place of the committed one's. */
const withVignettes = (vignettes: string) => committed.replace(/\[\[vignette\]\][\s\S]*?(?=\n\[end\])/, vignettes);

/** The nine vignettes of the spec's table (§3), as reel.toml lists them. */
const NINE = [
  ["cold-open", null, 6, 5],
  ["point", "Point at anything", 6.5, null],
  ["version", "A new version, in seconds", 6.5, null],
  ["realtime", "Realtime collaboration", 7, null],
  ["history", "Every version kept", 6, null],
  ["devserver", "Comment on your dev server", 6.5, false],
  ["more", "Pages that do more", 6.5, false],
  ["claxbot", "ClaxBot builds a page", 7, false],
  ["share", "Share to clax.dev", 7, null],
] as const;
const nineToml = NINE.map(([id, title, seconds, short]) => [
  "[[vignette]]",
  `id = "${id}"`,
  ...(title ? [`title = "${title}"`] : []),
  `seconds = ${seconds}`,
  ...(typeof short === "number" ? [`short_seconds = ${short}`] : short === false ? ["in_short = false"] : ["in_short = true"]),
  "",
].join("\n")).join("\n");

describe("parseReel", () => {
  it("parses the realtime table", () => {
    const cfg = parseReel(committed, ["agent", "realtime"]);
    expect(cfg.fps).toBe(30);
    expect(cfg.size).toEqual([1920, 1080]);
    expect(cfg.capture).toEqual({ viewport: [1280, 800], scale: 1.25, theme: "light" });
    expect(cfg.vignettes[0]!.title).toBe("Realtime collaboration");
    expect(cfg.vignettes[0]!.inShort).toBe(true);
    expect(cfg.poster).toEqual({ vignette: "realtime", at: 3.5 });
    expect(cfg.upload).toEqual({ landingArtifact: "", landing: "" });
  });

  it("names only vignettes that have a script", () => {
    expect(() => parseReel(committed, scripted)).not.toThrow();
  });

  it("refuses an unknown key", () => {
    expect(() => parseReel(committed.replace("[reel]\n", "[reel]\ncolour = 1\n"), ["agent", "realtime"])).toThrow(/unknown key reel\.colour/);
    expect(() => parseReel(committed.replace("in_short = true", "in_short = true\nspeed = 2"), ["agent", "realtime"])).toThrow(/unknown key realtime\.speed/);
    expect(() => parseReel(`${committed}\n[extra]\n`, ["agent", "realtime"])).toThrow(/unknown key reel\.toml\.extra/);
  });

  it("refuses a vignette with no script", () => {
    expect(() => parseReel(committed, [])).toThrow(/realtime: no script/);
  });

  it("refuses a duration outside 3–12 s", () => {
    expect(() => parseReel(committed.replace("seconds = 7", "seconds = 13"), ["agent", "realtime"])).toThrow(/realtime\.seconds/);
    expect(() => parseReel(committed.replace("seconds = 7", "seconds = 2.5"), ["agent", "realtime"])).toThrow(/realtime\.seconds/);
  });

  it("refuses a title on the cold open, and none elsewhere", () => {
    const open = withVignettes('[[vignette]]\nid = "cold-open"\ntitle = "Hello"\nseconds = 6\n\n[[vignette]]\nid = "realtime"\ntitle = "Realtime collaboration"\nseconds = 7\n');
    expect(() => parseReel(open, ["cold-open", "realtime"])).toThrow(/cold-open: the cold open has no title card/);
    expect(() => parseReel(committed.replace('title = "Realtime collaboration"\n', ""), ["agent", "realtime"])).toThrow(/realtime: no title/);
  });

  it("refuses a landing that does not end in its artifact ID", () => {
    const up = (id: string, url: string) => committed.replace(/landing_artifact = ""/, `landing_artifact = "${id}"`).replace(/landing = ""/, `landing = "${url}"`);
    expect(parseReel(up("abc123", "https://clax.dev/a/abc123"), ["agent", "realtime"]).upload.landingArtifact).toBe("abc123");
    expect(() => parseReel(up("abc123", "https://clax.dev/a/xyz789"), ["agent", "realtime"])).toThrow(/upload\.landing/);
    expect(() => parseReel(up("abc123", ""), ["agent", "realtime"])).toThrow(/upload: set landing_artifact and landing together/);
  });

  it("checks the GIF's cut list", () => {
    const seg = (v: string, s: number) => `[[gif.segment]]\nvignette = "${v}"\nbeat = "tick"\noffset = -1.0\nseconds = ${s}\ncaption = "x"\n`;
    const gif = (segs: string) => `${committed}\n[gif]\nfps = 15\nsize = [720, 450]\ncaption_px = 36\nmax_mib = 2\n\n${segs}`;
    expect(parseReel(gif(seg("realtime", 3) + seg("realtime", 4)), ["agent", "realtime"]).gif!.segments).toHaveLength(2);
    expect(() => parseReel(gif(seg("realtime", 8)), ["agent", "realtime"])).toThrow(/gif\.segment: 1 segments/);
    expect(() => parseReel(gif(seg("realtime", 6) + seg("realtime", 6)), ["agent", "realtime"])).toThrow(/gif\.segment seconds in all/);
    expect(() => parseReel(gif(seg("realtime", 3) + seg("point", 4)), ["agent", "realtime"])).toThrow(/no vignette point/);
  });
});

describe("totalSeconds", () => {
  it("totals the cuts", () => {
    const cfg = parseReel(withVignettes(nineToml), NINE.map(v => v[0]));
    const full = totalSeconds(cfg, "full");
    const short = totalSeconds(cfg, "short");
    expect(full).toBeGreaterThanOrEqual(60);
    expect(full).toBeLessThanOrEqual(80);
    expect(short).toBeGreaterThanOrEqual(40);
    expect(short).toBeLessThanOrEqual(50);
    // 6 + 53 of vignettes + 8 cards of 1.2 + 4; 5 + 33 + 5 cards of 1.0 + 4.
    expect(full).toBe(72.6);
    expect(short).toBe(47);
    expect(() => checkCuts(cfg)).not.toThrow();
  });

  it("refuses cuts outside their ranges", () => {
    expect(() => checkCuts(parseReel(committed, ["agent", "realtime"]))).toThrow(/the full cut: 25\.4 s is outside 60–80 s/);
  });
});
