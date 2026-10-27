// Reads and checks web/reel/reel.toml (spec 2026-10-27-sizzle-reel-design
// §9.3): the reel's vignettes in order, their titles and durations, the
// cuts, the end card, the poster frame, the README GIF's cut list and the
// landing artifact. Anything the file does not define is refused by name,
// so a typo fails here rather than rendering a reel without it.
import { parse } from "smol-toml";

export type VignetteCfg = { id: string; title?: string; seconds: number; shortSeconds?: number; inShort: boolean };
export type GifSegment = { vignette: string; beat: string; offset: number; seconds: number; caption: string };
export type ReelCfg = {
  fps: number;
  size: [number, number];
  capture: { viewport: [number, number]; scale: number; theme: "light" | "dark" };
  cardSeconds: { full: number; short: number };
  vignettes: VignetteCfg[];
  end: { seconds: number; headline: string; footer: string };
  poster: { vignette: string; at: number };
  gif?: { fps: number; size: [number, number]; captionPx: number; maxMib: number; segments: GifSegment[] };
  upload: { landingArtifact: string; landing: string };
};

/** The vignette that opens the reel: the one with no title card. */
export const COLD_OPEN = "cold-open";
/** How long a vignette may run, in seconds, in either cut. */
export const VIGNETTE_SECONDS = [3, 10] as const;
/** How long each cut may run in all, in seconds (`checkCuts`). */
export const CUT_SECONDS = { full: [60, 80], short: [40, 50] } as const;
/** How long the README GIF's segments may run in all, and how many there are. */
export const GIF_SECONDS = [6, 10] as const;
export const GIF_SEGMENTS = [2, 4] as const;

type Table = Record<string, unknown>;

const isTable = (v: unknown): v is Table => typeof v === "object" && v !== null && !Array.isArray(v) && !(v instanceof Date);

/** Fails on any key of `t` (at `path`) not in `allowed`. */
function only(t: Table, path: string, allowed: string[]) {
  for (const k of Object.keys(t)) if (!allowed.includes(k)) throw new Error(`unknown key ${path}.${k}`);
}

function table(t: Table, key: string, path: string): Table {
  const v = t[key];
  if (!isTable(v)) throw new Error(`${path}: missing table`);
  return v;
}

function num(t: Table, key: string, path: string): number {
  const v = t[key];
  if (typeof v !== "number" || !Number.isFinite(v)) throw new Error(`${path}.${key}: want a number, got ${JSON.stringify(v)}`);
  return v;
}

function str(t: Table, key: string, path: string): string {
  const v = t[key];
  if (typeof v !== "string") throw new Error(`${path}.${key}: want a string, got ${JSON.stringify(v)}`);
  return v;
}

function pair(t: Table, key: string, path: string): [number, number] {
  const v = t[key];
  if (!Array.isArray(v) || v.length !== 2 || !v.every(x => typeof x === "number" && x > 0)) {
    throw new Error(`${path}.${key}: want two positive numbers, got ${JSON.stringify(v)}`);
  }
  return [v[0] as number, v[1] as number];
}

function within(v: number, [lo, hi]: readonly [number, number], what: string) {
  if (v < lo || v > hi) throw new Error(`${what}: ${v} s is outside ${lo}–${hi} s`);
}

function vignette(v: unknown, i: number, scripts: string[]): VignetteCfg {
  if (!isTable(v)) throw new Error(`vignette ${i + 1}: want a table`);
  const id = str(v, "id", `vignette ${i + 1}`);
  only(v, id, ["id", "title", "seconds", "short_seconds", "in_short"]);
  if (!scripts.includes(id)) throw new Error(`${id}: no script (web/reel/vignettes/${id}.ts)`);
  const title = v.title === undefined ? undefined : str(v, "title", id);
  if (id === COLD_OPEN && title !== undefined) throw new Error(`${id}: the cold open has no title card, so no title`);
  if (id !== COLD_OPEN && !title) throw new Error(`${id}: no title (its title card's line)`);
  const seconds = num(v, "seconds", id);
  within(seconds, VIGNETTE_SECONDS, `${id}.seconds`);
  const shortSeconds = v.short_seconds === undefined ? undefined : num(v, "short_seconds", id);
  if (shortSeconds !== undefined) within(shortSeconds, VIGNETTE_SECONDS, `${id}.short_seconds`);
  if (v.in_short !== undefined && typeof v.in_short !== "boolean") throw new Error(`${id}.in_short: want true or false`);
  // A vignette with a short length of its own is in the short cut.
  const inShort = (v.in_short as boolean | undefined) ?? shortSeconds !== undefined;
  return { id, title, seconds, shortSeconds, inShort };
}

function gif(g: Table, ids: string[]): NonNullable<ReelCfg["gif"]> {
  only(g, "gif", ["fps", "size", "caption_px", "max_mib", "segment"]);
  const raw = g.segment;
  if (!Array.isArray(raw)) throw new Error("gif.segment: want [[gif.segment]] tables");
  const segments = raw.map((s, i): GifSegment => {
    const at = `gif.segment ${i + 1}`;
    if (!isTable(s)) throw new Error(`${at}: want a table`);
    only(s, at, ["vignette", "beat", "offset", "seconds", "caption"]);
    const seg = { vignette: str(s, "vignette", at), beat: str(s, "beat", at), offset: num(s, "offset", at), seconds: num(s, "seconds", at), caption: str(s, "caption", at) };
    if (!ids.includes(seg.vignette)) throw new Error(`${at}.vignette: no vignette ${seg.vignette} in the reel`);
    if (seg.seconds <= 0) throw new Error(`${at}.seconds: want a positive length`);
    return seg;
  });
  const [lo, hi] = GIF_SEGMENTS;
  if (segments.length < lo || segments.length > hi) throw new Error(`gif.segment: ${segments.length} segments, want ${lo}–${hi}`);
  within(segments.reduce((n, s) => n + s.seconds, 0), GIF_SECONDS, "gif.segment seconds in all");
  return { fps: num(g, "fps", "gif"), size: pair(g, "size", "gif"), captionPx: num(g, "caption_px", "gif"), maxMib: num(g, "max_mib", "gif"), segments };
}

/**
 * The reel as `toml` defines it. `scripts` are the vignette IDs that have a
 * script (`web/reel/vignettes/<id>.ts`). Throws an Error naming the key or
 * the vignette ID at fault: an unknown key, a vignette with no script, a
 * title on the cold open or none on another vignette, a duration outside
 * 3–10 s, a poster or GIF segment naming a vignette the reel lacks, a GIF
 * cut list of the wrong length, or a `landing` that does not end in
 * `landing_artifact`. The cuts' totals are checked by `checkCuts`, once the
 * reel lists every vignette.
 */
export function parseReel(toml: string, scripts: string[]): ReelCfg {
  const doc = parse(toml) as Table;
  only(doc, "reel.toml", ["reel", "vignette", "end", "poster", "gif", "upload"]);

  const reel = table(doc, "reel", "reel");
  only(reel, "reel", ["fps", "size", "capture", "card_seconds"]);
  const capture = table(reel, "capture", "reel.capture");
  only(capture, "reel.capture", ["viewport", "scale", "theme"]);
  const theme = str(capture, "theme", "reel.capture");
  if (theme !== "light" && theme !== "dark") throw new Error(`reel.capture.theme: want "light" or "dark", got ${JSON.stringify(theme)}`);
  const cards = table(reel, "card_seconds", "reel.card_seconds");
  only(cards, "reel.card_seconds", ["full", "short"]);

  if (!Array.isArray(doc.vignette) || doc.vignette.length === 0) throw new Error("vignette: want one [[vignette]] table per vignette");
  const vignettes = doc.vignette.map((v, i) => vignette(v, i, scripts));
  const ids = vignettes.map(v => v.id);
  const twice = ids.find((id, i) => ids.indexOf(id) !== i);
  if (twice) throw new Error(`${twice}: listed twice`);

  const end = table(doc, "end", "end");
  only(end, "end", ["seconds", "headline", "footer"]);

  const poster = table(doc, "poster", "poster");
  only(poster, "poster", ["vignette", "at"]);
  const posterOf = vignettes.find(v => v.id === poster.vignette);
  if (!posterOf) throw new Error(`poster.vignette: no vignette ${JSON.stringify(poster.vignette)} in the reel`);
  const at = num(poster, "at", "poster");
  if (at < 0 || at > posterOf.seconds) throw new Error(`poster.at: ${at} s is outside ${posterOf.id}'s 0–${posterOf.seconds} s`);

  const upload = table(doc, "upload", "upload");
  only(upload, "upload", ["landing_artifact", "landing"]);
  const landingArtifact = str(upload, "landing_artifact", "upload");
  const landing = str(upload, "landing", "upload");
  if ((landingArtifact === "") !== (landing === "")) throw new Error("upload: set landing_artifact and landing together, or neither");
  if (landing && new URL(landing).pathname.replace(/\/+$/, "").split("/").pop() !== landingArtifact) {
    throw new Error(`upload.landing: ${landing} does not end in landing_artifact ${landingArtifact}`);
  }

  return {
    fps: num(reel, "fps", "reel"),
    size: pair(reel, "size", "reel"),
    capture: { viewport: pair(capture, "viewport", "reel.capture"), scale: num(capture, "scale", "reel.capture"), theme },
    cardSeconds: { full: num(cards, "full", "reel.card_seconds"), short: num(cards, "short", "reel.card_seconds") },
    vignettes,
    end: { seconds: num(end, "seconds", "end"), headline: str(end, "headline", "end"), footer: str(end, "footer", "end") },
    poster: { vignette: posterOf.id, at },
    gif: doc.gif === undefined ? undefined : gif(table(doc, "gif", "gif"), ids),
    upload: { landingArtifact, landing },
  };
}

/** How long cut `cut` runs, in seconds: each vignette in it, a title card
 * before each one with a title, and the end card. */
export function totalSeconds(cfg: ReelCfg, cut: "full" | "short"): number {
  const inCut = cut === "full" ? cfg.vignettes : cfg.vignettes.filter(v => v.inShort);
  const card = cut === "full" ? cfg.cardSeconds.full : cfg.cardSeconds.short;
  const body = inCut.reduce((n, v) => n + (cut === "short" ? v.shortSeconds ?? v.seconds : v.seconds) + (v.title ? card : 0), 0);
  return Math.round((body + cfg.end.seconds) * 1000) / 1000;
}

/** Fails when either cut runs outside its range (`CUT_SECONDS`): the
 * composition's check, once reel.toml lists every vignette. */
export function checkCuts(cfg: ReelCfg): void {
  for (const cut of ["full", "short"] as const) within(totalSeconds(cfg, cut), CUT_SECONDS[cut], `the ${cut} cut`);
}
