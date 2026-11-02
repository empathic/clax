// The routes' JSON, as the API contract has it: every type here is
// generated (contract.gen.ts, from web/sdk/openapi.json, which the Rust
// routes generate: spec 2026-10-27-clax-sdk-design D2, §4.1). Field names
// are the wire's. Change a type in Rust, then `just contract`.

import type { Operations, Report, ReportComment } from "./contract.gen";

export type {
  Anchor,
  AnchorArea,
  AnchorKind,
  AnchorRect,
  ApiErrorBody,
  Badge,
  CaptureInfo,
  CaptureMethod,
  CaptureMode,
  ContractVersion,
  DiagLine,
  Discovery,
  Environment,
  ErrorCode,
  FixAttempt,
  FixOutcome,
  FlagValue,
  Lease,
  Operations,
  ProposerKind,
  Queue,
  QueueRole,
  Release,
  Report,
  ReportArtifact,
  ReportClaim,
  ReportComment,
  ReportContext,
  ReportersSee,
  ReporterProfile,
  ReporterToken,
  ReporterTokenRequest,
  ReportPage,
  ReportState,
  ResolveReason,
  ServerStamp,
  ServiceGrantCreated,
} from "./contract.gen";

/** A report's thread status. */
export type ReportStatus = Report["status"];
/** Who wrote a comment on a report. */
export type AuthorKind = ReportComment["author_kind"];

/** The error codes the queue routes answer. */
export type QueueErrorCode = {
  [K in keyof Operations]: Operations[K]["path"] extends `/api/queues${string}` ? Operations[K]["errors"] : never;
}[keyof Operations];
