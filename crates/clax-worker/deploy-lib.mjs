// What deploy.mjs decides from what it reads, as pure functions, so they are
// tested without an account (deploy-lib.test.mjs).

/** Whether a failed `wrangler deploy`'s output (stdout and stderr) says
 * only its triggers were left half-done: the script was uploaded, but the
 * routes or custom domains attached to it were "only partially updated"
 * (a Cloudflare API error such as code 10013 on the custom-domains call).
 * Running the same deploy again finishes them; any other failure is not
 * this one. */
export function partialTriggerUpdate(text) {
  const s = String(text ?? "");
  if (!/\bpartially updated\b/i.test(s)) return false;
  return /\btrigger configuration\b/i.test(s) || /\bcustom[ -]?domains?\b/i.test(s);
}

/** A build identity unique to one upload, which deploy.mjs gives the Worker
 * as `CLAX_BUILD` and `/healthz` reports as `build`: the commit's first
 * twelve characters (when known), the UTC time and four random bytes, e.g.
 * `0a2f4805a1b2-20261023T101500Z-9f86d081`. */
export function buildId(commit, now = new Date(), random = "") {
  const at = now.toISOString().replace(/[-:]/g, "").replace(/\.\d+Z$/, "Z");
  return [commit ? commit.slice(0, 12) : "", at, random].filter(Boolean).join("-");
}

/** What a 200 from `/healthz` says while waiting for build `want`:
 * `{ done: true }` once it reports that build, else `{ waiting }` naming
 * what answers instead (an instance from before this deploy). */
export function healthOf(body, want) {
  let j = null;
  try {
    j = JSON.parse(body);
  } catch {
    // not JSON
  }
  if (!j || typeof j !== "object") return { waiting: `/healthz answered something that is not its JSON (${String(body).slice(0, 80)}); waiting for build ${want}` };
  if (j.build === want) return { done: true };
  const was = j.build ? `build ${j.build}` : "a build that reports none";
  return { waiting: `the instance from before this deploy (${was}, started ${j.started_at ?? "?"}) still answers; waiting for build ${want}` };
}
