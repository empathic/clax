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

/** Where "Your reports" shows: `OverlayOptions.reports`. */
export type ReportsMode = "button" | "off";

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
export type ProposerKind = "agent" | "person";
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
  /** Absent from a server older than the field: read it as `agent`. */
  proposer_kind?: ProposerKind;
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
  /** The queue's ID. Optional with an install, whose config names it. */
  queue?: string;
  /** The install's ID, whose config Clax serves (its queue, wording, and whether it shows "Your reports" and the element picker). Default `<meta name="clax-install">`'s content. The overlay starts once the config is fetched, else on this page's own options. */
  install?: string;
  /** Gets a reporter token for the reporter's public key, from the deployer's backend. */
  auth: ReporterAuth;
  /** `"corner"` (the default): Report and "Your reports" in the bottom right corner; `false`: none. */
  launcher?: "corner" | false;
  /** Whether this reporter may file reports (default true). */
  canFile?: boolean;
  /** `"button"` (the default): "Your reports" beside Report; `"off"`: no "Your reports", panel or polling. Default `<meta name="clax-reports">`'s content. */
  reports?: ReportsMode;
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
  /** A stable ID of the signed-in user, in the host's own terms (hashed
   * before it is stored). With it, the reporter's key (a non-extractable
   * CryptoKey) and its reporter token are kept in IndexedDB for this
   * instance, queue and user and reused across page loads until shortly
   * before the token expires, so `auth` is called about once per 15
   * minutes per user rather than on every page load. Without it, every
   * page load asks `auth` for a token. */
  user?: string;
  /** The plugins (`@empathic/clax-client-sdk/plugins`); default the classic set. */
  plugins?: ClaxPlugin[];
  /** How the composer is shown: a presentation plugin's id. */
  presentation?: string;
  /** `--clax-*` custom properties: the site's look. */
  theme?: Record<string, string>;
  /** Filters and attachments turned on or off (by id), over each plugin's own default. */
  defaults?: Record<string, boolean>;
}

/** The plugin contract (web/sdk/src/plugin.ts), version 1. */
export type Capability = "dom:read" | "dom:render" | "media:display" | "storage:local" | "net:host";
export interface SentRow { id: string; label: string; preview: string; bytes?: number; optIn: boolean; default: boolean; privacyNote?: string }
export interface Box { x: number; y: number; w: number; h: number }
export interface CaptureContribution { id: string; label: string; method: "dom" | "display"; supported?(win: Window): boolean; take(req: unknown): Promise<unknown> }
export interface AnnotateTool { id: string; label: string; key?: string }
export interface AnnotateContribution { tools: AnnotateTool[]; ui?: unknown }
export interface FilterInput { png: Blob; at: Box; scale: number; doc: Document }
export interface FilterOutput { png: Blob; note: string | null }
export interface FilterContribution { id: string; label: string; default: boolean; apply(input: FilterInput): Promise<FilterOutput> }
export interface AttachmentData { diagnostics?: DiagLine[]; release?: Release; environment?: Partial<Environment>; app?: Record<string, unknown> }
export interface Collected { data: AttachmentData; row: SentRow | null }
export interface AttachmentContribution { id: string; label: string; default: boolean; collect(ctx: { doc: Document }): Promise<Collected> | Collected }
export interface PickerContribution { id: string; label: string; key?: string; mode?: "element" | "area" | "screen"; refine?(target: Element | Range | null, doc: Document): Record<string, unknown> | null }
export type Layout = "dock" | "dialog" | "drawer" | "sheet" | "window";
export interface HandoffRequest { mode: "element" | "area" | "screen" | "note"; url: string; title: string }
export interface PresentationContribution { id: string; label: string; layout: Layout; available?(win: Window): boolean; fallback?: string; handoff?(req: HandoffRequest, win: Window): boolean }
export interface Contributions {
  capture?: CaptureContribution[];
  annotate?: AnnotateContribution;
  filters?: FilterContribution[];
  attachments?: AttachmentContribution[];
  pickers?: PickerContribution[];
  presentations?: PresentationContribution[];
  theme?: { tokens: Record<string, string> };
  adapter?: { name: string };
  self?: { instance: string; queue: string };
}
export interface BusEvents {
  pick: { mode: string; target: Element | Range | null };
  compose: { body: string };
  "before-send": { body: string; rows: SentRow[] };
  sent: { id: string };
  reply: { id: string };
  error: { message: string };
}
export type BusMessage<E extends keyof BusEvents> = BusEvents[E] & { readonly event: E; cancel(reason: string): void };
export interface PluginContext {
  on<E extends keyof BusEvents>(event: E, fn: (m: BusMessage<E>) => void): () => void;
  granted: readonly Capability[];
}
export interface ClaxPlugin {
  name: string;
  version: string;
  apiVersion: number;
  capabilities: Capability[];
  contributes: Contributions;
  setup?(ctx: PluginContext): void | (() => void);
}

/** What a running overlay tells its host. */
export interface OverlayEvents {
  badge: number;
  filed: Report;
  /** Whether a report is under way (the mode bar, a pick, or the form open);
   * a listener added later hears the current value at once. */
  active: boolean;
}

/** A running overlay. */
export interface OverlayHandle {
  report(opts?: { mode?: Mode | "note" }): void;
  showReports(): void;
  /** What the host's command palette may offer: Send feedback, and Your reports. */
  actions(): { id: string; label: string; run(): void }[];
  on(event: "badge", fn: (n: number) => void): () => void;
  on(event: "filed", fn: (r: Report) => void): () => void;
  on(event: "active", fn: (on: boolean) => void): () => void;
  close(): void;
  /** Forgets the reporter, as the host does when its user signs out:
   * deletes the key and token kept for `user` and drops them from this
   * overlay, which keeps nothing more. */
  forget(): Promise<void>;
}

/** Whose kept key and token `forgetReporter` deletes. */
export interface ReporterRef {
  instance: string;
  queue: string;
  user: string;
}

/** Forgets the key and token kept for `r.user` (the `user` option): call
 * it on sign-out; it needs no running overlay. */
export declare function forgetReporter(r: ReporterRef): Promise<void>;

/** How long the host's `context` and `metadata` functions may take, in ms. */
export declare const CONTEXT_TIMEOUT_MS: 2000;

/** Starts recording the page's recent errors before the overlay starts. */
export declare const diagnostics: { install(win?: Window): Recorder };

/** Starts the overlay on `doc` (default this page's). */
export declare function overlay(opts: OverlayOptions, doc?: Document): OverlayHandle;

declare global {
  interface Window {
    /** The overlay's classic script (`/_clax/overlay.js`). */
    Clax: { overlay: typeof overlay; diagnostics: typeof diagnostics; CONTEXT_TIMEOUT_MS: typeof CONTEXT_TIMEOUT_MS; forgetReporter: typeof forgetReporter };
  }
}
