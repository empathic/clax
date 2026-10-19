// The inspect view's answers as the app hears them (`host.ts`, "The inspect
// view"): only the fields and sizes each message names, whatever else the
// page sent. Loaded with the page's first answer, so app mode's host chunk
// carries none of it.
import type { InspectError } from "../../../bridge/src/protocol";
import { type HostMessage, INSPECT_ERROR_CHARS } from "./host";

/** What an inspection carries at most, whatever the page sends. */
const INSPECT_ERRORS = 20;
const INSPECT_TEXT_CHARS = 3072;
const INSPECT_TITLE_CHARS = 200;
const INSPECT_KINDS = new Set(["error", "unhandledrejection", "console"]);
/** What a test run's answer carries at most. */
const TEST_FAILED = 50;
const TEST_NAME_CHARS = 201;

const str = (v: unknown, n: number) => (typeof v === "string" ? v.slice(0, n) : "");

/** The base64 of `buf`. */
function base64(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf);
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

/** The host message for the page's answer `m` to an inspection: only the
 * fields and sizes `inspection` names, whatever else the page sent. */
export function inspected(m: Record<string, unknown>, maxBytes: number): HostMessage {
  if (typeof m.error === "string") return { type: "inspection", error: m.error.slice(0, INSPECT_ERROR_CHARS) || "the inspection failed" };
  const png = m.png;
  if (!(png instanceof ArrayBuffer)) return { type: "inspection", error: "the page sent no screenshot" };
  if (png.byteLength > maxBytes) return { type: "inspection", error: `the screenshot is over ${maxBytes} bytes` };
  const errors = (Array.isArray(m.errors) ? m.errors : [])
    .filter((e): e is InspectError => !!e && typeof e === "object" && INSPECT_KINDS.has(e.kind) && typeof e.text === "string")
    .slice(-INSPECT_ERRORS)
    .map(e => ({ kind: e.kind, text: e.text.slice(0, INSPECT_ERROR_CHARS) }));
  const failed = (Array.isArray(m.failed) ? m.failed : []).filter((u): u is string => typeof u === "string").slice(-INSPECT_ERRORS).map(u => u.slice(0, INSPECT_ERROR_CHARS));
  const height = typeof m.height === "number" && Number.isFinite(m.height) && m.height >= 0 ? m.height : 0;
  return { type: "inspection", png: base64(png), errors, failed, text: str(m.text, INSPECT_TEXT_CHARS), title: str(m.title, INSPECT_TITLE_CHARS), height };
}

const count = (v: unknown) => (typeof v === "number" && Number.isInteger(v) && v >= 0 ? v : null);

/** The host message for the page's answer `m` to a test run: only the fields
 * and sizes `tests` names, whatever else the page sent. */
export function testsMessage(m: Record<string, unknown>): HostMessage {
  if (typeof m.error === "string") return { type: "tests", error: m.error.slice(0, INSPECT_ERROR_CHARS) || "the test run failed" };
  const passed = count(m.passed);
  const skipped = count(m.skipped);
  if (passed === null || skipped === null || !Array.isArray(m.failed)) return { type: "tests", error: "the page sent no test results" };
  const failed = m.failed
    .filter((f): f is { name: string; error: string } => !!f && typeof f === "object" && typeof f.name === "string" && typeof f.error === "string")
    .slice(0, TEST_FAILED)
    .map(f => ({ name: f.name.slice(0, TEST_NAME_CHARS), error: f.error.slice(0, INSPECT_ERROR_CHARS) }));
  return { type: "tests", passed, failed, skipped };
}

/** The host message for the page's answer `m` to a migration run: only
 * `ok` or `error`, whatever else the page sent. */
export function migrationMessage(m: Record<string, unknown>): HostMessage {
  if (m.ok === true) return { type: "migration", ok: true };
  if (m.ok === false && typeof m.error === "string") return { type: "migration", ok: false, error: m.error.slice(0, INSPECT_ERROR_CHARS) || "the migration failed" };
  return { type: "migration", ok: false, error: "the page sent no migration result" };
}
