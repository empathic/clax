// The report-queue routes' JSON, written by hand until the contract is
// generated from the routes (spec 2026-10-27 §4.1). Field names are the
// wire's.

/** A queue's `reporters_see`: what a reporter token may read. */
export type ReportersSee = "own" | "all";
/** A member's role on a queue. */
export type QueueRole = "admin" | "triage";
/** A report's thread status. */
export type ReportStatus = "open" | "resolved";
/** A report's place in triage (spec 2026-10-29-feedback-loop-design §6.1):
 * `open`, waiting for someone; `claimed`, while a live lease holds it;
 * `fix_proposed`, while a fix waits for the reporter's verdict; `verified`,
 * once the reporter said it is fixed; `resolved`, once triage closed it
 * without a verdict. `verified` and `resolved` resolve its thread. */
export type ReportState = "open" | "claimed" | "fix_proposed" | "verified" | "resolved";
/** Why a `verified` or `resolved` report was closed. */
export type ResolveReason = "fixed" | "duplicate" | "wontfix" | "cannot_reproduce" | "other";
/** What became of a fix attempt: waiting for a verdict, verified, rejected
 * (the reporter reopened), superseded by a later fix, or closed (the report
 * was resolved before a verdict). */
export type FixOutcome = "pending" | "verified" | "rejected" | "superseded" | "closed";
/** Who wrote a comment on a report. */
export type AuthorKind = "viewer" | "agent" | "reporter";

/** `Queue`: what `GET /api/queues` lists, with the caller's role. */
export interface Queue {
  id: string;
  name: string;
  origins: string[];
  reporters_see: ReportersSee;
  created_at: string;
  role: QueueRole;
}

/** The external artifact a report is filed on. */
export interface ReportArtifact {
  id: string;
  external_id: string;
  title: string;
  url: string;
  metadata: unknown;
  metadata_version: number;
  revision: string | null;
}

/** One comment on a report. A reopen's comment carries its capture: the
 * reopen's number (the filing being capture 0), whether a clip came with it
 * (`…/comments/<cid>/clip`) and how many diagnostics lines
 * (`…/diagnostics?capture=<n>`); each is absent on any other comment. */
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

/** A live claim on a report. */
export interface ReportClaim {
  holder_name: string;
  expires_at: string;
}

/** `Report`: a thread filed into a queue, on an external artifact. */
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
  /** What the reporter's browser said at capture; absent when none was sent. */
  context?: ReportContext;
  /** The receiving server's stamp; absent on a report filed without one. */
  server?: ServerStamp;
  /** The reporter's profile, served to triage members only. */
  reporter_profile?: ReporterProfile;
  /** Why a `verified` or `resolved` report was closed. */
  resolve_reason?: ResolveReason;
  /** The fixes proposed on it, oldest first; absent when none was. */
  fixes?: FixAttempt[];
  /** How many times it was reopened; absent when never. */
  reopen_count?: number;
  /** How many diagnostics lines its latest capture holding any has; absent when none has. */
  diagnostics_lines?: number;
  /** How many diagnostics lines the filing itself (capture 0) has; absent when none. */
  filing_diagnostics_lines?: number;
}

/** What the reporter's browser said at capture (spec
 * 2026-10-29-feedback-loop-design §5.1): at most 16 KiB as compact JSON,
 * with no top-level key but these (else 400 `invalid_context`). Clax
 * scrubs `environment.url`, `environment.route`, `environment.title` and
 * every string of `app` again on receipt. */
export interface ReportContext {
  /** The shape's version: 1. */
  v: 1;
  /** When the capture was made, RFC 3339. */
  captured_at: string;
  release?: Release;
  /** The host app's feature flags at capture. */
  flags?: Record<string, FlagValue>;
  environment?: Environment;
  capture?: CaptureInfo;
  /** Opaque JSON the host app adds. */
  app?: unknown;
}

/** The release of the host app the reporter ran, as the host names it. */
export interface Release {
  id?: string;
  version?: string;
  commit?: string;
  channel?: string;
}

/** A feature flag's value. The SDK leaves out a flag whose number is not
 * finite (`NaN`, `Infinity`), which JSON cannot carry. */
export type FlagValue = boolean | number | string;

/** The page and browser a capture was made in. `url` keeps its query keys,
 * never their values. */
export interface Environment {
  url?: string;
  route?: string;
  title?: string;
  /** CSS pixels, and the device pixel ratio. */
  viewport?: { w: number; h: number; dpr: number };
  /** The page's scroll offsets in CSS pixels. */
  scroll?: { x: number; y: number };
  user_agent?: string;
  platform?: string;
  locale?: string;
  time_zone?: string;
  /** As the page's `prefers-color-scheme` matched. */
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
  /** What a DOM render left out (`iframe`, `canvas`, `video`, a masked element, …), and where, in CSS pixels. */
  omitted?: { kind: string; rect: { x: number; y: number; w: number; h: number } }[];
}

/** What the server that received a report says about itself, as its
 * `/healthz` reports it, and when it received the report. */
export interface ServerStamp {
  version: string;
  commit?: string;
  build?: string;
  received_at: string;
}

/** Who a reporter is, as the integrator's backend asserts it when minting
 * its token: each field at most 256 bytes, `avatar_url` an `https:` or
 * `http:` URL (else 400 `invalid_profile`). */
export interface ReporterProfile {
  name?: string;
  email?: string;
  /** A GitHub login, for asking the reporter to review a fix. */
  github?: string;
  avatar_url?: string;
}

/** A fix proposed on a report, numbered from 1 in the order proposed,
 * with its verdict once given. */
export interface FixAttempt {
  n: number;
  /** Plain text, posted to the report as a comment the reporter sees. */
  summary: string;
  /** The change: a pull request, a commit page, a ticket. */
  url?: string;
  commit?: string;
  /** The release the fix ships in. */
  release?: string;
  /** Where the reporter checks the fix. */
  verify_url?: string;
  /** When the reporter can check it, such as `release:0.6.1`. */
  verify_after?: string;
  proposer_name: string;
  created_at: string;
  outcome: FixOutcome;
  verdict_note?: string;
  /** The verdict-giver's browser context at the verdict. */
  verdict_context?: ReportContext;
  verdict_at?: string;
}

/** A line of a report's diagnostics (spec 2026-10-29-feedback-loop-design
 * §5.2): `kind` `error` is an uncaught error, `rejection` an unhandled
 * promise rejection, `request` a failed request. Clax scrubs `text` again
 * on receipt. */
export interface DiagLine {
  /** When it happened, RFC 3339, at most 64 bytes. */
  at: string;
  level: "debug" | "info" | "warn" | "error";
  kind: "console" | "error" | "rejection" | "request";
  text: string;
}

/** `GET /api/queues/<qid>/reporter/badge`: the reports waiting on the
 * reporter (a fix waiting for its verdict, or a comment not its own since
 * it last read them), oldest first; `waiting` is `report_ids.length`. */
export interface Badge {
  waiting: number;
  report_ids: string[];
}

/** `Lease`: what a claim or a renewal answers. */
export interface Lease {
  lease_id: string;
  expires_at: string;
  fence: number;
  report: Report;
}

/** A page of reports; `next` is the cursor of the following page. */
export interface ReportPage {
  reports: Report[];
  next: string | null;
}

/** `POST /api/queues/{qid}/reporter-tokens`'s body. */
export interface ReporterTokenRequest {
  reporter_id: string;
  origin: string;
  /** The reporter key's 32 public bytes in unpadded base64url. */
  public_key: string;
  /** Who the backend says the reporter is; each report filed under the token keeps it. */
  profile?: ReporterProfile;
}

/** A reporter token: `cxr_…`, bound to a key, an origin and a reporter ID. */
export interface ReporterToken {
  token: string;
  expires_at: string;
}

/** `POST /api/queues/{qid}/service-grants`'s answer: the token and key are shown once. */
export interface ServiceGrantCreated {
  grant: { id: string; name: string; created_at: string };
  token: string;
  /** PKCS#8 of the grant's Ed25519 key, unpadded base64url. */
  private_key: string;
}

/** `GET /.well-known/clax`. */
export interface Discovery {
  kind: "local" | "hosted" | "worker";
  contract_version: string;
  sign_in: string[];
  capabilities: string[];
}

/** Every error answer's body (`ApiError`). */
export interface ApiErrorBody {
  error: { code: string; message: string };
}

/** Error codes the queue routes answer. */
export type QueueErrorCode =
  | "lease_lost"
  | "thread_taken"
  | "external_artifact"
  | "wrong_origin"
  | "token_expired"
  | "not_a_member"
  | "not_fix_proposed"
  | "not_claimable"
  | "invalid_context"
  | "invalid_diagnostics"
  | "invalid_profile"
  | "invalid_fix"
  | "invalid_query";
