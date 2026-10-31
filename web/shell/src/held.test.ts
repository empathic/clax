import { afterEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "./api";
import { heldOf } from "./held";
import { Refused, setRights } from "./people/api";
import { answerQuestion } from "./q/api";

const URL = "http://localhost:7480/inbox?confirm=cf_01";
const pending = () => new Response(JSON.stringify({ status: "confirmation_pending", confirmation_id: "cf_01", confirm_url: URL, decision: "change a person's rights" }), {
  status: 202,
  headers: { "content-type": "application/json", "x-clax-confirmation": "cf_01" },
});
const stub = (r: () => Response) => vi.stubGlobal("fetch", vi.fn(async () => r()));

afterEach(() => { vi.unstubAllGlobals(); });

describe("a decision held for the owner's confirmation", () => {
  it("is read as where to confirm, leaving the answer unread", async () => {
    const res = pending();
    expect(await heldOf(res)).toEqual({ id: "cf_01", url: URL, message: `Confirm in your browser: ${URL}` });
    expect(res.bodyUsed).toBe(false);
    expect(await heldOf(new Response(JSON.stringify({ status: "approval_pending" }), { status: 202 }))).toBeNull();
    expect(await heldOf(new Response("{}", { status: 200 }))).toBeNull();
  });

  it("makes a question's answer say where to confirm, not a result", async () => {
    stub(pending);
    const err = (await answerQuestion("Q1", { answers: [{ selected: ["Yes"], text: null }] }).catch((e: unknown) => e)) as ApiError;
    expect(err).toBeInstanceOf(ApiError);
    expect(err.code).toBe("confirmation_pending");
    expect(err.message).toContain(`Confirm in your browser: ${URL}`);
  });

  it("makes a person's rights change say where to confirm", async () => {
    stub(pending);
    const err = (await setRights("p1", "clax:reader").catch((e: unknown) => e)) as Refused;
    expect(err).toBeInstanceOf(Refused);
    expect([err.status, err.code, err.message]).toEqual([202, "confirmation_pending", `Confirm in your browser: ${URL}`]);
  });
});
