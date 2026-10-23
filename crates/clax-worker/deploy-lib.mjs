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
