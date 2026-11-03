// The Cloudflare API calls deploy.mjs and teardown.mjs make that wrangler
// has no command for: a site domain's wildcard DNS record, the preview
// bucket's lifecycle rule, and a preview's teardown (its routes, custom
// domains, DNS record and Worker). Each call takes an API token
// (CLOUDFLARE_API_TOKEN) and answers the API's `result`, or throws naming
// the call and the API's errors.
//
// The base is https://api.cloudflare.com/client/v4, or CLOUDFLARE_API_BASE
// (test-teardown.mjs points it at a fake). The shapes are Cloudflare's:
//   zones          https://developers.cloudflare.com/api/resources/zones/methods/list/
//   DNS records    https://developers.cloudflare.com/api/resources/dns/subresources/records/
//   Worker routes  https://developers.cloudflare.com/api/resources/workers/subresources/routes/
//   custom domains https://developers.cloudflare.com/api/resources/workers/subresources/domains/
//   scripts        https://developers.cloudflare.com/api/resources/workers/subresources/scripts/
//   R2 lifecycle   https://developers.cloudflare.com/api/resources/r2/subresources/buckets/subresources/lifecycle/

const BASE = () => (process.env.CLOUDFLARE_API_BASE || "https://api.cloudflare.com/client/v4").replace(/\/$/, "");

/** A client for the API token `token`. */
export function cloudflare(token) {
  if (!token) throw new Error("no Cloudflare API token (CLOUDFLARE_API_TOKEN)");

  /** `method` `path` (with `body` as JSON): the API's `result`. A 404 is
   * `null` when `missingOk`. */
  async function call(method, path, { body, missingOk = false } = {}) {
    const res = await fetch(`${BASE()}${path}`, {
      method,
      headers: { authorization: `Bearer ${token}`, ...(body === undefined ? {} : { "content-type": "application/json" }) },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(30_000),
    });
    if (res.status === 404 && missingOk) return null;
    let j = null;
    try {
      j = await res.json();
    } catch {
      // not JSON
    }
    if (!res.ok || j?.success === false) {
      const errors = (j?.errors ?? []).map((e) => `${e.message} [code: ${e.code}]`).join("; ");
      throw new Error(`Cloudflare API ${method} ${path}: HTTP ${res.status}${errors ? `: ${errors}` : ""}`);
    }
    return j?.result ?? null;
  }

  /** Every page of a listing at `path` (which takes `page` and `per_page`). */
  async function all(path) {
    const out = [];
    for (let page = 1; ; page++) {
      const sep = path.includes("?") ? "&" : "?";
      const res = await fetch(`${BASE()}${path}${sep}page=${page}&per_page=100`, {
        headers: { authorization: `Bearer ${token}` },
        signal: AbortSignal.timeout(30_000),
      });
      const j = await res.json().catch(() => null);
      if (!res.ok || j?.success === false) {
        const errors = (j?.errors ?? []).map((e) => `${e.message} [code: ${e.code}]`).join("; ");
        throw new Error(`Cloudflare API GET ${path}: HTTP ${res.status}${errors ? `: ${errors}` : ""}`);
      }
      const items = j?.result ?? [];
      out.push(...items);
      const pages = j?.result_info?.total_pages;
      if (!items.length || (pages !== undefined && page >= pages) || (pages === undefined && items.length < 100)) return out;
    }
  }

  const q = encodeURIComponent;
  return {
    /** The ID of the zone named `name`; throws when the token sees none. */
    async zoneId(name) {
      const zones = await call("GET", `/zones?name=${q(name)}`);
      if (!zones?.length) throw new Error(`the zone ${name} is not on an account this token can read`);
      return zones[0].id;
    },
    /** The DNS records in `zone` named `name` (exactly). */
    dnsRecords: (zone, name) => all(`/zones/${zone}/dns_records?name=${q(name)}`),
    /** Makes a proxied AAAA record `name` → 100:: in `zone` (the address
     * Cloudflare's docs give for a hostname a Worker alone answers). */
    createProxiedRecord: (zone, name, comment) =>
      call("POST", `/zones/${zone}/dns_records`, { body: { type: "AAAA", name, content: "100::", proxied: true, ttl: 1, comment } }),
    deleteDnsRecord: (zone, id) => call("DELETE", `/zones/${zone}/dns_records/${id}`, { missingOk: true }),
    /** The Worker routes on `zone`: `{ id, pattern, script }`. */
    routes: (zone) => call("GET", `/zones/${zone}/workers/routes`),
    deleteRoute: (zone, id) => call("DELETE", `/zones/${zone}/workers/routes/${id}`, { missingOk: true }),
    /** The custom domains attached to the Worker `service`. */
    workerDomains: (account, service) => all(`/accounts/${account}/workers/domains?service=${q(service)}`),
    deleteWorkerDomain: (account, id) => call("DELETE", `/accounts/${account}/workers/domains/${id}`, { missingOk: true }),
    /** The account's Workers: `{ id, ... }`, `id` being the script name. */
    scripts: (account) => call("GET", `/accounts/${account}/workers/scripts`),
    /** Deletes the Worker `name` with its Durable Objects (`force`); a
     * Worker that is gone already is no error. */
    deleteScript: (account, name) => call("DELETE", `/accounts/${account}/workers/scripts/${q(name)}?force=true`, { missingOk: true }),
    /** The R2 bucket's lifecycle rules. */
    lifecycle: (account, bucket) => call("GET", `/accounts/${account}/r2/buckets/${q(bucket)}/lifecycle`),
    /** Replaces the R2 bucket's lifecycle rules with `rules`. */
    putLifecycle: (account, bucket, rules) => call("PUT", `/accounts/${account}/r2/buckets/${q(bucket)}/lifecycle`, { body: { rules } }),
  };
}

/** The lifecycle rule that deletes every object of the preview bucket
 * `days` days after it was written. */
export function expiryRule(days) {
  return {
    id: `clax-preview-expire-${days}d`,
    enabled: true,
    conditions: { prefix: "" },
    deleteObjectsTransition: { condition: { type: "Age", maxAge: days * 86400 } },
  };
}

/** Whether `rules` (a bucket's lifecycle rules) already hold `rule`, by ID
 * and content. */
export function hasRule(rules, rule) {
  return (rules ?? []).some((r) => r.id === rule.id && JSON.stringify(r.deleteObjectsTransition) === JSON.stringify(rule.deleteObjectsTransition) && r.enabled);
}
