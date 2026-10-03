// The `sample` capability's shell side, in the owner's browser only (the
// host offers `sample` only with the token): consent before the first call of
// the view (Grants: one dialog, an allow that is never stored), the call over
// `POST /api/artifacts/<aid>/sample` with the bearer token, each SSE frame
// relayed to the page as `clax:event {ns: "sample", topic: "frame", data:
// {call, event, data}}`, cancellation, page tool results, and the top bar's
// call count. Every refusal reaches the page as one of sample.d.ts's codes.
// This module is a lazy chunk (registry.ts), loaded on the page's first call.
import { readSse } from "../sse";
import { CapError } from "./errors";
import type { HandlerFactory } from "./host";

/** sample.d.ts's `SampleErrorCode`: the only codes a page may see. */
const SAMPLE_CODES = new Set(["invalid_request", "prompt_too_large", "images_unavailable", "tools_unavailable", "image_rejected", "cancelled", "not_granted", "session_expired", "sampling_disabled", "not_declared", "rate_limited", "refused", "empty_completion", "invalid_json", "upstream_error", "capability_disabled", "capability_removed", "transform_error", "queue_overflow"]);

/** A refusal before the stream, as the page may see it: a token the daemon
 * no longer takes (it restarted) is `session_expired`, a missing artifact
 * `not_declared`, a body over the limit `prompt_too_large`, and any code
 * outside the contract (`forbidden`, `forbidden_origin`, `timeout`) `upstream_error`. */
export async function capErrorFrom(res: Response): Promise<CapError> {
  let code = "";
  let message = `HTTP ${res.status}`;
  try {
    const e = (await res.json()).error as { code?: unknown; message?: unknown };
    code = String(e?.code ?? "");
    if (typeof e?.message === "string") message = e.message;
  } catch { /* not JSON */ }
  if (res.status === 401) return new CapError("session_expired", "the page's session with Clax ended; reload the page");
  if (res.status === 404) return new CapError("not_declared", message);
  if (res.status === 413) return new CapError("prompt_too_large", message);
  return new CapError(SAMPLE_CODES.has(code) ? code : "upstream_error", message);
}

export const sampleHandler: HandlerFactory = (env, grants) => {
  const controllers = new Map<string, AbortController>();
  const cancelled = new Set<string>();
  const daemonIds = new Map<string, string>();
  const auth = { authorization: `Bearer ${env.token ?? ""}` };
  const frame = (call: string, event: string, data: unknown) =>
    env.post({ type: "clax:event", ns: "sample", topic: "frame", data: { call, event, data } });

  async function run(call: string, req: unknown): Promise<null> {
    if (grants.state("sample") === "prompt") await grants.request(["sample"]);
    if (cancelled.has(call)) { cancelled.delete(call); return null; }
    if (grants.state("sample") !== "granted") throw new CapError("not_granted", "the viewer has not allowed this page to use Claude");
    const ctl = new AbortController();
    controllers.set(call, ctl);
    try {
      let res: Response;
      try {
        res = await fetch(`/api/artifacts/${env.aid}/sample`, { method: "POST", headers: { "content-type": "application/json", ...auth }, body: JSON.stringify(req), signal: ctl.signal });
      } catch (e) {
        if (ctl.signal.aborted) return null;
        throw new CapError("upstream_error", e instanceof Error ? e.message : String(e));
      }
      if (!res.ok || !res.body) throw await capErrorFrom(res);
      try {
        for await (const ev of readSse(res.body)) {
          let data: Record<string, unknown>;
          try { data = JSON.parse(ev.data) as Record<string, unknown>; } catch { continue; }
          if (ev.event === "start") {
            if (typeof data.call_id === "string") daemonIds.set(call, data.call_id);
            if (typeof data.calls_today === "number") env.onSampleCalls?.(data.calls_today, typeof data.daily_call_cap === "number" ? data.daily_call_cap : null);
          }
          frame(call, ev.event, data);
          if (ev.event === "done" || ev.event === "error") return null;
        }
        if (!ctl.signal.aborted) frame(call, "error", { code: "upstream_error", message: "the answer stopped before it finished" });
      } catch (e) {
        if (!ctl.signal.aborted) frame(call, "error", { code: "upstream_error", message: e instanceof Error ? e.message : String(e) });
      }
      return null;
    } finally {
      controllers.delete(call);
    }
  }

  /** The page went away: every call stops, and the daemon drops the provider request. */
  function end() {
    for (const c of controllers.values()) c.abort();
    controllers.clear();
    daemonIds.clear();
    cancelled.clear();
  }

  return {
    async call(method, args) {
      switch (method) {
        case "run":
          return run(String(args[0]), args[1]);
        case "cancel": {
          const call = String(args[0]);
          const ctl = controllers.get(call);
          if (ctl) ctl.abort(); else cancelled.add(call);
          daemonIds.delete(call);
          return null;
        }
        case "toolResult": {
          const [call, id, content, isError] = args;
          const daemonId = daemonIds.get(String(call)) ?? String(call);
          const res = await fetch(`/api/artifacts/${env.aid}/sample/${encodeURIComponent(daemonId)}/tool_result`, {
            method: "POST", headers: { "content-type": "application/json", ...auth },
            body: JSON.stringify({ id, content, is_error: isError === true }),
          });
          if (!res.ok && res.status !== 404) throw await capErrorFrom(res);
          return null;
        }
        case "limits":
          return (await env.sampleStatus?.())?.limits ?? { maxPromptBytes: 65536 };
        default:
          throw new CapError("capability_removed", `sample.${method} is not part of this runtime`);
      }
    },
    reset: end,
    leave: end,
    dispose: end,
  };
};
