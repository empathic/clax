// The inspect view's handlers (`host.ts`, "The inspect view" and "The
// migration view"): `inspect`, `runTests`, `runMigration`, `journalAck` and
// `restoreJournal`, the page's answers to them, and the live migration's
// write journal. A chunk of its own, which `host.ts` loads at install in the
// inspect view and elsewhere only on a call to one of them, so app mode's
// host chunk carries none of it. The answers keep only the fields and sizes
// each message names, whatever else the page sent.
import type { InspectError } from "../../../bridge/src/protocol";
import { getToken } from "../api";
import { replayJournal } from "../caps/db";
import { CapError } from "../caps/errors";
import type { DbJournal } from "../caps/host";
import type { ArtifactController } from "../view/artifact-controller";
import type { ClaxHost, HostMessage } from "./host";

/** `inspect`'s defaults and bounds (`bridge/src/inspect.ts` has the same). */
export const INSPECT_SETTLE_MS = 3000;
const INSPECT_SETTLE_MAX_MS = 10_000;
export const INSPECT_MAX_BYTES = 1024 * 1024;
const INSPECT_MAX_BYTES_MAX = 16 * 1024 * 1024;
/** The longest error an answer of the inspect view carries. */
export const INSPECT_ERROR_CHARS = 501;
/** `runTests`' bounds, and what its answer carries at most. */
export const TEST_FILES = 50;
const TEST_FILE = /^tests\/[A-Za-z0-9_\-./]{1,200}\.test\.js$/;
const TEST_MS_MAX = 30_000;
const TEST_TOTAL_MS_MAX = 120_000;
/** `runMigration`'s bounds. */
export const MIGRATE_MS = 30_000;
const MIGRATE_MS_MAX = 120_000;
const MIGRATE_FILE = /^[A-Za-z0-9_][A-Za-z0-9_\-./]{0,196}\.js$/;
/** The most entries one `restoreJournal` takes. */
const JOURNAL_MAX = 20_000;
const isVersion = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v) && v >= 1;
const bounded = (v: unknown, lo: number, hi: number, dflt: number) => (typeof v === "number" && Number.isFinite(v) ? Math.min(hi, Math.max(lo, Math.round(v))) : dflt);

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

/** The inspect view's handlers over view `ctl`, posting with `post` (which
 * adds the nonce): `inspecting` and `migrating` say which view this is,
 * `given` whether the app's `init` has come. */
export type InspectorContext = { ctl: ArtifactController; post(m: HostMessage): void; inspecting: boolean; migrating: boolean; given(): boolean };
export type Inspector = Pick<ClaxHost, "inspect" | "runTests" | "runMigration" | "journalAck" | "restoreJournal"> & {
  /** The page's answer `m` (`clax:inspection`, `clax:tests` or
   * `clax:migration`), already checked to come from the frame. */
  answered(m: Record<string, unknown> & { type: string }): void;
};

export function inspector({ ctl, post, inspecting, migrating, given }: InspectorContext): Inspector {
  // The inspection asked of the page, until it answers.
  let inspection: { requestId: string; maxBytes: number } | null = null;
  let inspections = 0;
  // The test run asked of the page, until it answers.
  let testRun: string | null = null;
  // The migration run asked of the page, until it answers.
  let migrationRun: string | null = null;
  // The live migration's write journal: open from a journaled run's start
  // to its answer; the writes waiting for the app's ack, by sequence.
  let journalOpen = false;
  let journalSeq = 0;
  const journalWaits = new Map<number, { resolve(): void; reject(e: CapError): void }>();
  // Set by the first journaled run, and kept for the document's life: once
  // that run has answered, every write fails.
  const journalOf: DbJournal = e => new Promise<void>((resolve, reject) => {
    if (!journalOpen) {
      reject(new CapError("unavailable", "writes are closed in this view"));
      return;
    }
    const seq = ++journalSeq;
    journalWaits.set(seq, { resolve, reject });
    post({ type: "journal", seq, path: e.path, before: e.before });
  });
  return {
    answered(m) {
      if (m.type === "clax:inspection") {
        if (!inspection || m.requestId !== inspection.requestId) return;
        const { maxBytes } = inspection;
        inspection = null;
        post(inspected(m, maxBytes));
      } else if (m.type === "clax:tests") {
        if (testRun === null || m.requestId !== testRun) return;
        testRun = null;
        post(testsMessage(m));
      } else if (m.type === "clax:migration") {
        if (migrationRun === null || m.requestId !== migrationRun) return;
        migrationRun = null;
        // The run is over: a write after it (a migration still going) fails.
        journalOpen = false;
        for (const wait of journalWaits.values()) wait.reject(new CapError("unavailable", "the migration has ended"));
        journalWaits.clear();
        post(migrationMessage(m));
      }
    },
    inspect(p) {
      if (!given()) return;
      if (!inspecting) { post({ type: "inspection", error: "this view is not the inspect view" }); return; }
      if (!ctl.gate.open) { post({ type: "inspection", error: "the page has not loaded" }); return; }
      const settleMs = bounded(p?.settleMs, 0, INSPECT_SETTLE_MAX_MS, INSPECT_SETTLE_MS);
      const maxBytes = bounded(p?.maxBytes, 1, INSPECT_MAX_BYTES_MAX, INSPECT_MAX_BYTES);
      inspection = { requestId: `inspect-${++inspections}`, maxBytes };
      ctl.send({ type: "clax:inspect", requestId: inspection.requestId, settleMs, maxBytes });
    },
    runTests(p) {
      if (!given()) return;
      if (!inspecting) { post({ type: "tests", error: "this view is not the inspect view" }); return; }
      const files = p?.files;
      if (!Array.isArray(files) || !files.length || files.length > TEST_FILES || !files.every(f => typeof f === "string" && TEST_FILE.test(f) && !f.split("/").includes(".."))) {
        post({ type: "tests", error: `files is a list of 1 to ${TEST_FILES} paths tests/<name>.test.js` });
        return;
      }
      if (!ctl.gate.open) { post({ type: "tests", error: "the page has not loaded" }); return; }
      testRun = `test-${++inspections}`;
      ctl.send({ type: "clax:test", requestId: testRun, files: [...files], testMs: bounded(p.testMs, 1, TEST_MS_MAX, 5000), totalMs: bounded(p.totalMs, 1, TEST_TOTAL_MS_MAX, 30_000) });
    },
    runMigration(p) {
      if (!given()) return;
      const refuse = (error: string) => post({ type: "migration", ok: false, error });
      if (!migrating) { refuse("this view is not the migration view"); return; }
      const { file, from, to, totalMs } = (p && typeof p === "object" ? p : {}) as Partial<Record<"file" | "from" | "to" | "totalMs", unknown>>;
      if (typeof file !== "string" || !MIGRATE_FILE.test(file) || file.split("/").some(seg => seg === "" || seg === "." || seg === "..") || !isVersion(from) || !isVersion(to)) {
        refuse("file is a path <name>.js in the app, from and to are versions");
        return;
      }
      if (!ctl.gate.open) { refuse("the page has not loaded"); return; }
      migrationRun = `migrate-${++inspections}`;
      if (p?.journal === true) {
        journalOpen = true;
        ctl.dbJournal = journalOf;
      }
      ctl.send({ type: "clax:migrate", requestId: migrationRun, file, from, to, totalMs: bounded(totalMs, 1, MIGRATE_MS_MAX, MIGRATE_MS) });
    },
    journalAck(p) {
      const seq = p?.seq;
      const wait = typeof seq === "number" ? journalWaits.get(seq) : undefined;
      if (!wait) return;
      journalWaits.delete(seq as number);
      if (p.ok === true) wait.resolve();
      else wait.reject(new CapError("unavailable", "the update could not keep this document's value"));
    },
    restoreJournal(p) {
      if (!given()) return;
      const fail = (error: string) => post({ type: "journalRestored", ok: false, error });
      if (!migrating) { fail("this view is not the migration view"); return; }
      const entries = p?.entries;
      const isData = (v: unknown) => v === null || (typeof v === "object" && !Array.isArray(v));
      if (!Array.isArray(entries) || entries.length > JOURNAL_MAX || !entries.every(e => e && typeof e === "object" && typeof e.path === "string" && isData(e.before))) {
        fail(`entries is a list of at most ${JOURNAL_MAX} {path, before}`);
        return;
      }
      void getToken().then(token => replayJournal(ctl.id, token, entries)).then(
        () => post({ type: "journalRestored", ok: true }),
        (e: unknown) => fail(e instanceof Error ? e.message.slice(0, INSPECT_ERROR_CHARS) : String(e)),
      );
    }
  };
}
