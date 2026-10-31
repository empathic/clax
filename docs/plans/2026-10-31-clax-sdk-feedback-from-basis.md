# SDK feedback from basis-clax's first integration

Status: feedback for the SDK's and Clax's owners, recorded, nothing built.
From personal-basis-clax-goal's first-pass write-up (2026-10-31, before
its cold reads), embedding the overlay in fold serve.

## What worked

- `Clax.overlay({instance, queue, auth, externalId, metadata})` and its
  `close()` handle.
- `auth(publicKey)` as the seam where the host's backend vouches for its
  user.
- `crates/clax-core/src/sig.rs`'s description of `clax-ed25519 v2` and its
  `sdk_vector_is_fixed` test vector were enough to reimplement the signer
  in Rust bit for bit. Keep that vector public and fixed.

## What was hard, with a possible answer for each

1. **Finding it.** No published package and no README route from outside
   the empathic repo; the first guess at the API was wrong on almost every
   field. *Possible answer:* publish @empathic/clax-sdk (a release needs
   Bobby), and link web/sdk/README.md from the instance's own page about
   queues.
2. **`/_clax/overlay.js` sends no `Access-Control-Allow-Origin`**
   (crates/clax-server/src/shell/mod.rs static_file's fallback), so a host
   must proxy it same-origin to load it with SRI. *Possible answer:* ACAO
   `*` on that one public, credential-free file, or the queue's origins.
   Server owner's call.
3. **Pinning.** No published hash per version and no versioned path (such
   as `/_clax/0.6.0/overlay.js`). *Possible answer:* serve a versioned path
   as immutable, and publish its SRI hash beside the release.
4. **No per-report target hook.** A host whose content has stable identity
   cannot put what a report is about on its anchor; the person picks it in
   comment mode only. *Possible answer:* a `target(node) => {about}` option,
   or a way to enrich the anchor (the `@empathic/clax-sdk/anchor` entry,
   held on goal/clax-sdk-kit, is where its types would come from). Overlaps
   changelist's gap 3 (open() with a host-chosen anchor).
5. **The service mint is a nine-line signature, and clax-core is
   GPL-2.0-only OR AGPL-3.0-only**, which a permissively licensed Rust host
   cannot depend on. *Possible answer:* a small Rust crate for the queue
   service client under the SDK's licence (LGPL-2.1-only) or a permissive
   one. Licence owner's call (Empathic).
6. **Theme.** The host token set (`theme`) exists only on the held branch
   (13037a30).

## The question it asked, answered from main d7812795

No HTML page on the instance lets a plain link (with JavaScript off)
comment on an artifact by its external ID. The page routes
(crates/clax-server/src/router.rs:701-725) are the shell's (`/queues`,
`/inbox`, `/a/{aid}` and so on), each a JavaScript app; none takes an
external ID. The nearest is `/a/<artifact ID>`: a report's
`artifact.id` (web/sdk/src/types.ts `ReportArtifact`) names the external
artifact Clax made for that external ID, and a member of the queue's
instance can open it there. A reporter has no page at all: the reporter
routes are signed API calls, which need WebCrypto.
