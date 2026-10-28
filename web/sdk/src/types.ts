// The report-queue routes' JSON, written by hand until the contract is
// generated from the routes (spec 2026-10-27 §4.1). Field names are the
// wire's.

/** A queue's `reporters_see`: what a reporter token may read. */
export type ReportersSee = "own" | "all";
/** A member's role on a queue. */
export type QueueRole = "admin" | "triage";
/** A report's thread status. */
export type ReportStatus = "open" | "resolved";
/** A report's place in triage: open, claimed by a live lease, or resolved. */
export type ReportState = "open" | "claimed" | "resolved";
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

/** One comment on a report. */
export interface ReportComment {
  id: string;
  author_kind: AuthorKind;
  author_name: string;
  body: string;
  created_at: string;
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
  | "not_a_member";
