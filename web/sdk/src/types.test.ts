import { describe, expect, it } from "vitest";
import codes from "../test/queue-error-codes.json";
import { ClaxError } from "./client";
import type { ClaxErrorCode, QueueErrorCode } from "./types";

// Every member of QueueErrorCode, and nothing else: TypeScript fails on a
// key missing from the union and on one outside it.
const EVERY: Record<QueueErrorCode, true> = {
  invalid_token: true, token_expired: true, invalid_signature: true, stale_request: true, replayed_request: true,
  forbidden_origin: true, wrong_origin: true, limit_reached: true, body_too_large: true, busy: true, timeout: true,
  invalid_origin: true, invalid_reporter_id: true, invalid_key: true, invalid_profile: true,
  invalid_args: true, invalid_multipart: true, invalid_anchor: true, invalid_metadata: true, metadata_too_large: true,
  invalid_clip: true, invalid_comment: true, invalid_context: true, invalid_diagnostics: true, invalid_query: true,
  external_artifact: true, not_found: true, not_fix_proposed: true,
  not_a_member: true, not_for_agents: true, not_claimable: true, claim_required: true, lease_lost: true,
  thread_taken: true, invalid_state: true, invalid_fix: true, no_session: true,
};

describe("QueueErrorCode", () => {
  it("is the list web/sdk/test/queue-error-codes.json holds, which Clax's reporter route tests check every refusal against", () => {
    expect(Object.keys(EVERY).sort()).toEqual([...(codes as string[])].sort());
    expect(new Set(codes as string[]).size).toBe((codes as string[]).length);
  });

  it("types ClaxError.code, which still takes any other string", () => {
    const e = new ClaxError("invalid_key", "refused", 400);
    const code: ClaxErrorCode = e.code;
    const other: ClaxErrorCode = "http_502";
    expect([code, other]).toEqual(["invalid_key", "http_502"]);
  });
});
