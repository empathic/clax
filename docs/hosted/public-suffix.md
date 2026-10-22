# The site domain on the Public Suffix List

Each artifact on clax.dev is served on a host of its own,
`<artifact ID>.clax.run`. To a browser those hosts are separate origins, but
not separate *sites* until `clax.run` is on the
[Public Suffix List](https://publicsuffix.org/) (PSL): until then every
`<ID>.clax.run` is same-site with every other, so

- one artifact's page can set a `Domain=clax.run` cookie that every other
  artifact's host receives;
- `SameSite` cookies do not separate two artifacts: a request from one
  artifact to another counts as same-site;
- browsers that key storage or caches by site put sibling artifacts in one
  partition.

The platform's own cookies (`__Host-clax_site`, `__Host-clax_visitor`) are
host-only and `__Host-` prefixed, so no sibling can set or overwrite them;
the risk left is to cookies and state an artifact's own page manages. Once
`clax.run` is a public suffix, each `<ID>.clax.run` is a registrable domain
and a site of its own, and that risk goes.

A self-hosted deployment with a site domain of its own
(`deploy.mjs --site-domain`, `[hosted] site_domain`) has the same choice to
make for its domain.

## What to file

The PSL takes entries for domains whose subdomains are run by different
parties, which artifacts are, in its **private** section.

1. Check the requirements in the list's
   [guidelines](https://github.com/publicsuffix/list/wiki/Guidelines): among
   them, the domain registered for at least two more years at the time of
   the request, and a reason that is a security boundary between
   subdomains (this one is).
2. Add the DNS record that proves control, before opening the pull request
   or as soon as it has a number: a `TXT` record at `_psl.clax.run` whose
   value is the pull request's URL,

   ```
   _psl.clax.run.  TXT  "https://github.com/publicsuffix/list/pull/<number>"
   ```

   On Cloudflare: the `clax.run` zone > DNS > Records > Add record, type
   `TXT`, name `_psl`. It stays as long as the entry is on the list.
3. Fork [github.com/publicsuffix/list](https://github.com/publicsuffix/list)
   and edit `public_suffix_list.dat`: in the private section
   (between `===BEGIN PRIVATE DOMAINS===` and `===END PRIVATE DOMAINS===`),
   add a block in its alphabetical place by organization name:

   ```
   // Empathic : https://clax.dev
   // Submitted by <name> <address>
   clax.run
   ```

   The entry is `clax.run` itself, not `*.clax.run`: the rule makes each
   first-level name under it a registrable domain.
4. Open the pull request with the template's questions answered: who runs
   the domain, why (each `<ID>.clax.run` is one person's artifact, with
   scripts of its own that must not share cookies or storage with the
   others), how long the domain is registered, and the `_psl` record.
5. Answer the maintainers' review on the pull request. Their queue takes
   weeks or more; nothing in Clax waits on it.

## How to check it shipped

The list itself: once merged, `clax.run` appears in
<https://publicsuffix.org/list/public_suffix_list.dat>:

```sh
curl -s https://publicsuffix.org/list/public_suffix_list.dat | grep -nx 'clax.run'
```

Browsers ship their own copy of the list and update it in later releases,
so the merge is not the end:

- Chrome and other Chromium browsers: Chromium's
  `net/base/registry_controlled_domains/effective_tld_names.dat`
  ([source search](https://source.chromium.org/search?q=clax.run%20file:effective_tld_names.dat));
  a release built after it lists `clax.run` has it.
- Firefox: mozilla-central's `netwerk/dns/effective_tld_names.dat`
  ([searchfox](https://searchfox.org/mozilla-central/search?q=clax.run&path=effective_tld_names.dat));
  Firefox can also receive a newer list between releases, so check in the
  browser too.
- Safari: WebKit uses the copy in Apple's operating systems, which updates
  with them; there is no public file to check, so check in the browser.

In a browser, from a page on one artifact's host (its devtools console):

```js
// Refused once clax.run is a public suffix: the cookie does not appear.
document.cookie = "psl_probe=1; Domain=clax.run; Path=/";
document.cookie.includes("psl_probe");   // false once shipped
```

and a request from one artifact's host to another's is sent with
`Sec-Fetch-Site: cross-site` once shipped, `same-site` before (the request
headers in the network panel, on a `fetch("https://<other ID>.clax.run/_clax/healthz")`
made from the first artifact's page).

Each browser that passes both checks treats every artifact as a site of its
own.
