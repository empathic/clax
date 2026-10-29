// Types of the embeddable Clax overlay, served beside it as
// `/_clax/overlay.d.ts`. The classic script `/_clax/overlay.js` defines
// `window.Clax`; the ES module `/_clax/overlay.mjs` exports the same names.
// A plain page, a React or a SvelteKit app starts it once, in the browser:
//
//   const clax = Clax.overlay({ instance, queue, auth, launcher: false, context });
//   clax.report({ mode: "screen" });
//   clax.on("badge", n => …);
//
// Kept equal to web/overlay/src/main.ts by web/overlay/src/dts-check.ts.

/** What the reporter captures: a pick, a drawn area, or the visible viewport. */
export type Mode = "element" | "area" | "screen";

/** A value, or a function that gives it (perhaps later). */
export type Given<T> = T | (() => T | Promise<T>);

/** A reporter token: `cxr_…`, bound to a key, an origin and a reporter ID. */
export interface ReporterToken {
  token: string;
  expires_at: string;
}

/** Gets a reporter token for the key `publicKey` (unpadded base64url) from
 * the deployer's own backend, which mints it with the queue's service grant. */
export type ReporterAuth = (publicKey: string) => Promise<ReporterToken>;

/** The release of the host app the reporter ran, as the host names it. */
export interface Release {
  id?: string;
  version?: string;
  commit?: string;
  channel?: string;
}

/** A feature flag's value. */
export type FlagValue = boolean | number | string;

/** The page and browser a capture was made in. */
export interface Environment {
  url?: string;
  route?: string;
  title?: string;
  viewport?: { w: number; h: number; dpr: number };
  scroll?: { x: number; y: number };
  user_agent?: string;
  platform?: string;
  locale?: string;
  time_zone?: string;
  color_scheme?: "light" | "dark";
}

/** What the reporter captured. */
export type CaptureMode = "element" | "range" | "area" | "screen";
/** How the clip was made: a render of the page's DOM, or the browser's screen capture. */
export type CaptureMethod = "dom" | "display";

/** How a report's clip was captured. */
export interface CaptureInfo {
  mode: CaptureMode;
  method: CaptureMethod;
  omitted?: { kind: string; rect: { x: number; y: number; w: number; h: number } }[];
}

/** What the reporter's browser said at capture: at most 16 KiB as JSON. */
export interface ReportContext {
  v: 1;
  captured_at: string;
  release?: Release;
  flags?: Record<string, FlagValue>;
  environment?: Environment;
  capture?: CaptureInfo;
  app?: unknown;
}

/** The host's part of a capture's context: only `release`, `flags` and `app` are taken. */
export type HostContext = Partial<ReportContext>;

export type ReportStatus = "open" | "resolved";
export type ReportState = "open" | "claimed" | "fix_proposed" | "verified" | "resolved";
export type ResolveReason = "fixed" | "duplicate" | "wontfix" | "cannot_reproduce" | "other";
export type FixOutcome = "pending" | "verified" | "rejected" | "superseded" | "closed";
export type AuthorKind = "viewer" | "agent" | "reporter";

export interface ReportArtifact {
  id: string;
  external_id: string;
  title: string;
  url: string;
  metadata: unknown;
  metadata_version: number;
  revision: string | null;
}

export interface ReportComment {
  id: string;
  author_kind: AuthorKind;
  author_name: string;
  body: string;
  created_at: string;
  capture?: number;
  has_clip?: boolean;
  diagnostics_lines?: number;
}

export interface ReportClaim {
  holder_name: string;
  expires_at: string;
}

export interface ServerStamp {
  version: string;
  commit?: string;
  build?: string;
  received_at: string;
}

export interface ReporterProfile {
  name?: string;
  email?: string;
  github?: string;
  avatar_url?: string;
}

export interface FixAttempt {
  n: number;
  summary: string;
  url?: string;
  commit?: string;
  release?: string;
  verify_url?: string;
  verify_after?: string;
  proposer_name: string;
  created_at: string;
  outcome: FixOutcome;
  verdict_note?: string;
  verdict_context?: ReportContext;
  verdict_at?: string;
}

/** A report filed into a queue, as Clax answers it. */
export interface Report {
  id: string;
  queue_id: string;
  artifact: ReportArtifact;
  anchor: unknown;
  status: ReportStatus;
  state: ReportState;
  reporter_id: string | null;
  metadata: unknown;
  metadata_version: number;
  has_clip: boolean;
  comments: ReportComment[];
  claim: ReportClaim | null;
  created_at: string;
  context?: ReportContext;
  server?: ServerStamp;
  reporter_profile?: ReporterProfile;
  resolve_reason?: ResolveReason;
  fixes?: FixAttempt[];
  reopen_count?: number;
  diagnostics_lines?: number;
}

/** A line of the page's recent diagnostics. */
export interface DiagLine {
  at: string;
  level: "debug" | "info" | "warn" | "error";
  kind: "console" | "error" | "rejection" | "request";
  text: string;
}

/** A window's diagnostics recorder. */
export interface Recorder {
  lines(): DiagLine[];
  count(): number;
  stop(): void;
}

/** `Clax.overlay`'s options. */
export interface OverlayOptions {
  /** The instance's base URL, such as `https://clax.corp.example`. */
  instance: string;
  /** The queue's ID. */
  queue: string;
  /** Gets a reporter token for the reporter's public key, from the deployer's backend. */
  auth: ReporterAuth;
  /** `"corner"` (the default): Report and "Your reports" in the bottom right corner; `false`: none. */
  launcher?: "corner" | false;
  /** Whether this reporter may file reports (default true). */
  canFile?: boolean;
  /** The host's `release`, `flags` and `app`, or a function giving them, called when a capture starts. */
  context?: Given<HostContext>;
  /** JSON attached to each report, or a function giving it, called once per report. */
  metadata?: Given<unknown>;
  /** Whether "Include recent errors" starts ticked (default false). */
  diagnostics?: { default?: boolean };
  /** The page's artifact; default `<meta name="clax-artifact">`'s content. */
  externalId?: string;
  /** The nonce the page's Content-Security-Policy gives its own inline
   * styles, on a page whose `style-src` refuses 'unsafe-inline' (or any
   * strict string, such as `"strict"`, where it uses hashes). The overlay
   * styles itself without inline style; given this, its page renders
   * (clips) are drawn without the page's web fonts from the first, as reading
   * them takes an inline <style> such a policy refuses, so the overlay trips
   * no violation. A policy in a `<meta http-equiv>` is read without it. */
  nonce?: string;
}

/** What a running overlay tells its host. */
export interface OverlayEvents {
  badge: number;
  filed: Report;
}

/** A running overlay. */
export interface OverlayHandle {
  report(opts?: { mode?: Mode }): void;
  showReports(): void;
  on(event: "badge", fn: (n: number) => void): () => void;
  on(event: "filed", fn: (r: Report) => void): () => void;
  close(): void;
}

/** How long the host's `context` and `metadata` functions may take, in ms. */
export declare const CONTEXT_TIMEOUT_MS: 2000;

/** Starts recording the page's recent errors before the overlay starts. */
export declare const diagnostics: { install(win?: Window): Recorder };

/** Starts the overlay on `doc` (default this page's). */
export declare function overlay(opts: OverlayOptions, doc?: Document): OverlayHandle;

declare global {
  interface Window {
    /** The overlay's classic script (`/_clax/overlay.js`). */
    Clax: { overlay: typeof overlay; diagnostics: typeof diagnostics; CONTEXT_TIMEOUT_MS: typeof CONTEXT_TIMEOUT_MS };
  }
}
