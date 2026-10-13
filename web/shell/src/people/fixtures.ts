// People fixtures for the page's unit tests, and a fetch stub that answers
// People's routes and keeps every request.
import { vi } from "vitest";
import type { PeopleAgent, PeopleGrant, PeoplePerson, Rights } from "../api";
import { PRESETS } from "./model";

export const COLLAB: Rights = PRESETS["clax:collaborator"];
export const grant = (o: Partial<PeopleGrant> = {}): PeopleGrant => ({
  id: "g1", status: "approved", via: "invite", client: { name: "sam-mbp", key: "SHA256:9f2cAbCd", peer_ip: "192.168.1.40" },
  person: { public_id: "p1", display_name: "Sam" }, owner: false, narrow: null, created_at: "2026-10-09T10:00:00Z",
  decided_at: "2026-10-09T10:00:00Z", last_used_at: "2026-10-09T11:00:00Z", ended_at: null, ended_by: null, ...o,
});
export const agent = (o: Partial<PeopleAgent> = {}): PeopleAgent => ({
  handle: "h1", name: "clax-multi-person", harness: "codex", person: "p1", grant_id: "g1", started_at: "2026-10-09T10:01:00Z",
  narrow: null, elevate: null, rights: COLLAB, ...o,
});
export const person = (o: Partial<PeoplePerson> = {}): PeoplePerson => ({
  public_id: "p1", display_name: "Sam", revoked_at: null, rights: COLLAB, grants: [grant()], agents: [agent()], ...o,
});

export type Call = { url: string; method: string; body: unknown };
/** Answers `routes[<METHOD> <path>]` (a value, or a function of the body;
 * a `[status, body]` pair for a refusal), else 404; keeps every request. */
export function stubFetch(routes: Record<string, unknown>) {
  const calls: Call[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    const method = init?.method ?? "GET";
    const body = init?.body ? JSON.parse(String(init.body)) : undefined;
    calls.push({ url, method, body });
    let r = routes[`${method} ${url}`];
    if (typeof r === "function") r = r(body);
    if (r === undefined) return new Response(JSON.stringify({ error: { code: "not_found", message: "not found" } }), { status: 404 });
    if (Array.isArray(r) && typeof r[0] === "number") return new Response(JSON.stringify(r[1]), { status: r[0] });
    return new Response(JSON.stringify(r));
  }));
  return calls;
}
/** Lets the page's fetches and their updates land. */
export async function settle() {
  const { flushSync } = await import("svelte");
  for (let i = 0; i < 6; i++) await new Promise(r => setTimeout(r, 0));
  flushSync();
}
