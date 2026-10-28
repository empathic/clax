// Checks the config deploy.mjs generates, without wrangler, a login or a
// build: `deploy.mjs --dry-run --config-only` against a temporary record
// directory (CLAX_DEPLOY_HOME) and `--out` file. The routes (the public host
// as a custom domain, the site domain as a wildcard route on its zone), the
// vars (SITE_DOMAIN, no CONTENT_ORIGIN; a record's content origin as
// RETIRED_CONTENT_ORIGIN and a custom domain), each refused site domain,
// and the record's site domain when none is given. Run by scripts/worker-checks.sh.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { HERE } from "./harness.mjs";
import { retiredContentOrigin, sanCovers, siteDomainOf, siteHealthy } from "./deploy.mjs";

const home = mkdtempSync(join(tmpdir(), "clax-deploy-config-"));
let n = 0;
let failures = 0;

/** Runs deploy.mjs's config-only dry run with `args`; answers its exit
 * status, its output and the config it wrote (or null). */
function deploy(args) {
  const out = join(home, `wrangler-${++n}.jsonc`);
  const r = spawnSync(process.execPath, [join(HERE, "deploy.mjs"), "--dry-run", "--config-only", "--out", out, ...args], {
    cwd: HERE,
    env: { ...process.env, CLAX_DEPLOY_HOME: home, CLOUDFLARE_ACCOUNT_ID: "" },
    stdio: ["ignore", "pipe", "pipe"],
    encoding: "utf8",
  });
  const config = existsSync(out) ? JSON.parse(readFileSync(out, "utf8").split("\n").filter((l) => !l.startsWith("//")).join("\n")) : null;
  return { status: r.status, text: `${r.stdout}${r.stderr}`, config };
}

function check(name, f) {
  try {
    f();
    console.log(`ok   ${name}`);
  } catch (e) {
    failures += 1;
    console.log(`FAIL ${name}: ${e.message}`);
  }
}

/** A refused run: non-zero, nothing written, the message matching `why`. */
function refused(args, why) {
  const r = deploy(args);
  assert.notEqual(r.status, 0, r.text);
  assert.equal(r.config, null, "wrote a config");
  assert.match(r.text, why);
}

try {
  check("the public host is a custom domain and the site domain a wildcard route on its zone", () => {
    const r = deploy(["--zone", "example.com", "--site-domain", "example.run"]);
    assert.equal(r.status, 0, r.text);
    assert.deepEqual(r.config.routes, [
      { pattern: "example.com", custom_domain: true },
      { pattern: "*.example.run/*", zone_name: "example.run" },
    ]);
    assert.equal(r.config.workers_dev, false);
    assert.equal(r.config.preview_urls, false);
  });

  check("SITE_DOMAIN replaces CONTENT_ORIGIN", () => {
    const r = deploy(["--public", "clax.example.com", "--site-domain", "https://Example.run/"]);
    assert.equal(r.status, 0, r.text);
    assert.equal(r.config.vars.SITE_DOMAIN, "example.run");
    assert.equal(r.config.vars.PUBLIC_ORIGIN, "https://clax.example.com");
    assert.equal(r.config.vars.HOSTED, "1");
    assert.equal(r.config.vars.CLAX_DIAG_ROUTES, "0");
    assert.equal("CONTENT_ORIGIN" in r.config.vars, false, JSON.stringify(r.config.vars));
  });

  check("CLAX_BUILD_COMMIT names the checked-out commit, or is absent without one", () => {
    const r = deploy(["--public", "clax.example.com", "--site-domain", "example.run"]);
    assert.equal(r.status, 0, r.text);
    const head = spawnSync("git", ["rev-parse", "HEAD"], { cwd: HERE, encoding: "utf8" });
    if (head.status === 0) assert.equal(r.config.vars.CLAX_BUILD_COMMIT, head.stdout.trim());
    else assert.equal("CLAX_BUILD_COMMIT" in r.config.vars, false, JSON.stringify(r.config.vars));
  });

  check("--content is refused, naming --site-domain", () => refused(["--public", "example.com", "--content", "content.example.com"], /--site-domain/));
  check("a wildcard site domain is refused", () => refused(["--public", "example.com", "--site-domain", "*.example.run"], /not a wildcard/));
  check("the public host as the site domain is refused", () => refused(["--public", "example.com", "--site-domain", "example.com"], /is the public host/));
  check("a site domain under the public host is refused", () => refused(["--public", "example.com", "--site-domain", "sites.example.com"], /under the public host/));
  check("a public host under the site domain is refused", () => refused(["--public", "clax.example.run", "--site-domain", "example.run"], /public host clax\.example\.run is under the site domain/));
  check("a site domain sharing a parent with the public host is refused", () => refused(["--public", "app.example.com", "--site-domain", "sites.example.com"], /shares example\.com .*registrable domain of its own/));
  check("an IP literal site domain is refused", () => refused(["--public", "example.com", "--site-domain", "192.0.2.1"], /IP address/));
  check("one trailing dot is the same site domain", () => {
    const r = deploy(["--public", "example.com.", "--site-domain", "example.run."]);
    assert.equal(r.status, 0, r.text);
    assert.equal(r.config.vars.SITE_DOMAIN, "example.run");
    assert.equal(r.config.vars.PUBLIC_ORIGIN, "https://example.com");
    assert.deepEqual(r.config.routes[1], { pattern: "*.example.run/*", zone_name: "example.run" });
  });
  check("no site domain, no record and no terminal is refused", () => refused(["--public", "fresh.example.com"], /Site domain/));

  check("the record's site domain is taken when none is given", () => {
    writeFileSync(join(home, "kept.example.com.json"), JSON.stringify({ public_origin: "https://kept.example.com", site_domain: "kept.run" }));
    const r = deploy(["--public", "kept.example.com"]);
    assert.equal(r.status, 0, r.text);
    assert.equal(r.config.vars.SITE_DOMAIN, "kept.run");
    assert.deepEqual(r.config.routes[1], { pattern: "*.kept.run/*", zone_name: "kept.run" });
  });

  check("a record's content origin becomes the retired one, a custom domain of its own", () => {
    writeFileSync(
      join(home, "old.example.com.json"),
      JSON.stringify({ public_origin: "https://old.example.com", site_domain: "old.run", content_origin: "https://content.old.example.com" }),
    );
    const r = deploy(["--public", "old.example.com"]);
    assert.equal(r.status, 0, r.text);
    assert.equal(r.config.vars.RETIRED_CONTENT_ORIGIN, "https://content.old.example.com");
    assert.equal("CONTENT_ORIGIN" in r.config.vars, false);
    assert.deepEqual(r.config.routes[2], { pattern: "content.old.example.com", custom_domain: true });
    const fresh = deploy(["--public", "kept.example.com"]);
    assert.equal("RETIRED_CONTENT_ORIGIN" in fresh.config.vars, false, "none without one");
    assert.equal(fresh.config.routes.length, 2);
    assert.equal(retiredContentOrigin({ retired_content_origin: "https://c.example.com" }), "https://c.example.com");
    for (const bad of ["", "http://c.example.com", "https://c.example.com/x", "nope", 7]) {
      assert.equal(retiredContentOrigin({ content_origin: bad }), null, String(bad));
    }
  });

  check("--retired-content-origin names the retired content host without a record, and replaces the record's", () => {
    const r = deploy(["--public", "norecord.example.com", "--site-domain", "norecord.run", "--retired-content-origin", "https://content.norecord.example.com"]);
    assert.equal(r.status, 0, r.text);
    assert.equal(r.config.vars.RETIRED_CONTENT_ORIGIN, "https://content.norecord.example.com");
    assert.deepEqual(r.config.routes[2], { pattern: "content.norecord.example.com", custom_domain: true });
    const over = deploy(["--public", "old.example.com", "--retired-content-origin", "https://content2.old.example.com"]);
    assert.equal(over.status, 0, over.text);
    assert.equal(over.config.vars.RETIRED_CONTENT_ORIGIN, "https://content2.old.example.com");
  });
  check("--retired-content-origin takes an https:// origin alone", () => refused(["--public", "kept.example.com", "--retired-content-origin", "https://c.example.com/x"], /--retired-content-origin takes an https:\/\/ origin/));

  check("a given site domain replaces the record's", () => {
    const r = deploy(["--public", "kept.example.com", "--site-domain", "other.run"]);
    assert.equal(r.status, 0, r.text);
    assert.equal(r.config.vars.SITE_DOMAIN, "other.run");
  });

  check("a site host is healthy only with the Worker's answer", () => {
    const res = (status, headers = {}) => ({ status, headers: new Headers(headers) });
    const ok = { "referrer-policy": "no-referrer", "content-type": "application/json" };
    assert.equal(siteHealthy(res(200, ok), '{"ok":true,"version":"0.9.0"}'), true);
    assert.equal(siteHealthy(res(200, {}), '{"ok":true,"version":"0.9.0"}'), false, "no Referrer-Policy");
    assert.equal(siteHealthy(res(200, ok), "<html>parked</html>"), false, "not JSON");
    assert.equal(siteHealthy(res(200, ok), '{"ok":true}'), false, "no version");
    assert.equal(siteHealthy(res(403, ok), '{"error":{"code":"forbidden_host"}}'), false, "forbidden_host");
    assert.equal(siteHealthy(res(302, ok), ""), false, "a redirect");
    assert.equal(siteHealthy(res(522, {}), "<html>error 522</html>"), false, "Cloudflare's error page");
  });

  check("siteDomainOf and sanCovers", () => {
    assert.equal(siteDomainOf("clax.run", "clax.dev"), "clax.run");
    assert.throws(() => siteDomainOf("clax.dev", "app.clax.dev"), /under the site domain/);
    assert.equal(sanCovers("DNS:example.run, DNS:*.example.run", "abcdefghjkmn.example.run"), true);
    assert.equal(sanCovers("DNS:example.run", "abcdefghjkmn.example.run"), false);
    assert.equal(sanCovers("DNS:*.example.run", "a.b.example.run"), false);
    assert.equal(sanCovers("DNS:*.example.run", "abcdefghjkmn.example.runx"), false);
  });
} finally {
  rmSync(home, { recursive: true, force: true });
}

if (failures) {
  console.log(`${failures} failed`);
  process.exit(1);
}
console.log("deploy config: all ok");
