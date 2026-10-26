# Quickstart onboarding: the Clax features its landing page needs

Status: draft for the owner's review (2026-10-26).

## 1. Intent

Clax's landing page is a quickstart artifact: an interactive flow chart
that takes a new person from signing in to their first comment round trip
(sign in, install or skip, link, attach an agent, fork, comment, send,
change). Building it surfaced gaps in Clax itself; the owner accepted
these, each tracked as an idea on the quickstart's `ideas.html` page (its
ID in brackets):

- A. Publish signs this Clax in when it is signed in nowhere
  [`publish-without-remote`].
- B. Sign-in returns the person to the page they started from
  [`signin-return`].
- C. A page's capabilities are declared in a manifest file, per version
  [`capability-manifest`].
- D. One permissions system: per artifact, approved per version, listed
  and revocable in one view [`shell-tour`, its permissions half].
- E. The `account` capability: a page asks the shell to sign its viewer in
  [`account-capability`].
- F. The `shell` capability: a page points at Clax's own controls
  [`shell-tour`].
- G. The quickstart ships with Clax and is forked into each new person's
  gallery [`landing-page`].
- H. A setting to always publish new artifacts to clax.dev
  [`always-publish`].

The quickstart's own source lives in the repository (`web/quickstart/`) so
G can ship it and the page can change with the features it uses.

## 2. Owner decisions (2026-10-26)

- D1. Any viewer signs in with their own account, not only the owner (§5).
- D2. Other capabilities may require a signed-in viewer by a declared
  requirement the platform enforces, not by pages checking a status.
- D3. Sign-in is wholly the shell's, "for security, privacy, and sanity
  reasons". A page may only ask; it never sees a code, URL or token.
- D4. Every capability that reaches past the page goes through one
  permissions system, as on iOS, Android and in the Clax app: per
  artifact, per version approvals.
- D5. What an artifact may ask for is declared statically, in an
  artifact-level manifest.
- D6. Linking needs no CLI: someone who installed only a marketplace
  plugin links from the browser.
- D7. For now, signing in happens when someone first wants to publish
  [`signin-at-install` declined].
- D8. Pages stay harness-agnostic: a page never learns which harness an
  agent runs in [`setup-state`].
- D9. Each new person's gallery gets a fork of the quickstart.

## 3. A. Publish signs in (done)

Commit `56515a02`. On a local daemon the top bar's Publish button always
shows; signed in nowhere it reads Publish to clax.dev, starts the device
sign-in (`POST /api/remotes`), opens the approval page in a new tab, shows
the code and polls, then asks to publish. B replaces the new tab with the
same-tab return.

## 4. B. Coming back after sign-in

`/auth/github?next=` cannot return to a local page:
`clax_http::site_access::safe_next` keeps `next` to a path on the public
origin. The device sign-in gets a return address instead.

- **Hosted.** `POST /api/hosted/device/code` takes an optional
  `return_to`, accepted only as a loopback URL: scheme `http`, host
  `localhost`, `127.0.0.1` or `[::1]`, any port, a path, at most 2048
  bytes, no userinfo or fragment; anything else is 400 `invalid_args`. It
  is stored with the device code (`device_codes.return_to`, migration 55) and served
  only in `GET /api/hosted/device/<code>` to the approving member. The
  approval page (`/device`) says "Approve, then go back to
  localhost:7480", and after Approve or Deny navigates there with
  `clax_signin=approved|denied` added to the query.
- **Local.** `POST /api/remotes` takes an optional `return_to`, a path on
  the daemon's own origin (checked with `safe_next`); the daemon makes it
  absolute against the request's loopback shell origin and passes it on.
  A public Clax that refuses the field (older, `deny_unknown_fields`) is
  retried without it, so sign-in still works with a new tab.
- **Shell.** Publish sends the person in the same tab, with `return_to`
  set to the artifact's path plus `publish=1`. On load, `publish=1` with
  `clax_signin=approved` polls the pending remote once and reopens the
  Publish dialog; `denied` says so. Both parameters are dropped from the
  address.

A loopback-only `return_to` is not an open redirect: it sends only the
approving person, only after they decide, and only to their own machine.

## 5. E. The `account` capability

Any Clax page can ask the shell to sign its viewer in to a public Clax
(clax.dev, or another hosted daemon), and learn whether they are signed
in.

### 5.1 Whose account

- **A local daemon.** An artifact's page is served on its own
  `<ID>.localhost` site, which resolves on that machine alone; a connected
  person's browser on another machine gets the inbox, People and comments,
  not the pages (`docs/usage.md`, "Use"). The only viewer of a page is the
  owner, in any of their browsers. Their account at a public Clax is the
  daemon's remote for that origin (`remotes`, one per origin). No
  per-viewer token store is needed, and the daemon never holds another
  person's token.
- **A hosted daemon.** Viewers are people signed in to it, or, on a site
  open to the world, visitors who are not. The viewer's account is their
  own session there.

So D1 holds everywhere without a new identity model.

### 5.2 The namespace

`ClaxExtensions.Account` in `web/contract/clax-extensions.d.ts`, declared
as `{"account": {}}` or `{"account": {"origins": ["https://clax.dev"]}}`.

```ts
namespace Account {
  type Status = {
    signedIn: boolean;
    /** Local: each connected remote. Hosted: this daemon, one entry. */
    accounts: { origin: string; login: string | null; name: string | null }[];
  };
  type SignInResult =
    | { status: "connected"; origin: string; login: string | null }
    | { status: "declined" | "denied" | "expired" };
  interface Account {
    status(): Promise<Status>;
    onChange(fn: (s: Status) => void): () => void;
    /** From a click handler. Local: origin defaults to https://clax.dev. */
    signIn(origin?: string): Promise<SignInResult>;
  }
}
```

Rejection codes: `no_gesture`, `not_declared` (origin outside `origins`),
`in_progress`, `invalid_argument`, `unavailable`, `session_expired`.

### 5.3 Local `signIn`

The shell shows a consent ("*<title>* asks to sign this Clax in to
*clax.dev* as you": Sign in / Not now; Not now resolves `declined`), then
runs B's same-tab sign-in with `return_to` the artifact's path plus
`account=1`. On return the shell polls once; the page, reloaded, sees
`status().signedIn` and `onChange` fires. A remote already connected for
that origin resolves `connected` at once, with no consent.

### 5.4 Hosted `signIn`

The shell asks "Sign in to *<host>* to use *<title>*?", then navigates
the top window to `/auth/github?next=<the artifact's path>` (or
`/signin?next=`). A viewer already signed in resolves `connected` at once.

### 5.5 Who gets what

| viewer | `use("account")` | `signIn` |
|---|---|---|
| local owner's browser | yes | device sign-in (§5.3) |
| hosted, signed in | yes, `signedIn: true` | resolves `connected` |
| hosted, world visitor not signed in | yes, `signedIn: false` | navigates to sign-in (§5.4) |
| the Clax app's viewer | yes | the app's own Connect flow |
| outside the viewer | `null` | — |

### 5.6 Declared requirement (D2)

Each capability in `web/shell/src/caps/availability.ts` may carry
`needsAccount`; such a capability resolves `null` until this view's
`account.status().signedIn`, and a later `use()` after `onChange` resolves
it. The daemon enforces the same requirement on that capability's routes.
No existing capability sets it in v1.

## 6. C. The capability manifest

- An artifact's root may hold `clax.json`, `{"capabilities": {…}}` (other
  top-level keys refused, 400 `invalid_manifest`, so the file can grow
  deliberately). It is a file like any other: versioned, forked, pushed.
- Publish reads it. When a publish carries `clax.json`, its
  `capabilities` is the declaration, validated by
  `clax_core::capabilities::validate`; a `capabilities` argument that
  differs is 400 `manifest_mismatch`. When a publish carries a
  `capabilities` argument and no `clax.json`, the daemon writes
  `clax.json` into the version from it, so every new version has one.
  When neither is given, the previous version's `clax.json` carries
  forward as files do.
- The declaration is kept per version: migration 56 adds
  `versions.capabilities_json`; `artifacts.capabilities_json` stays the
  current version's, for the routes that read it. Restore takes the
  restored version's declaration (today it keeps the current one).
- Artifacts published before C have no `clax.json` until their next
  version.

## 7. D. One permissions system

- **Approval per version.** Migration 57 adds `capability_approvals
  (artifact_id, version_n, capabilities_json, approved_at, by)`: the
  declaration the owner approved, and at which version. A version whose
  declaration adds to the last approved one (new top-level names, or new
  `net` hosts: the Clax app's `addedCapabilities` rule in
  `apps/clax_app/lib/bundle/updater.dart`, ported) is not approved; until
  the owner approves it, the shell serves it the last approved
  declaration, and shows a bar: "v4 asks for: the internet
  (api.example.com), Claude. Review". The first version's declaration
  needs approval too, except when the owner or one of the owner's agents
  published it.
- **The view.** A Permissions panel in the artifact's more menu (owner
  only) lists what the page may use, with each grant's state and a
  revoke: `net` hosts (`net_grants`), Claude (`sample_grants`), other
  artifacts' data (`db_read_grants`), sign-in (`account`), Clax controls
  (`shell`), comments by the page. Revoke deletes the grant; the page
  asks again on its next use. It mirrors the Clax app's inspector
  (`apps/clax_app/lib/ui/app_inspector_permissions.dart`), which keeps
  its own UI.

## 8. F. The `shell` capability

`ClaxExtensions.Shell`, declared `{"shell": {}}`, served in the owner's
and editors' views:

- `highlight(control: "comment" | "threads" | "versions" | "publish" |
  "fork" | "send")` draws a coach mark on that control of the Clax window
  for 6 s or until clicked, and resolves `true`, or `false` when the
  control is not shown (a phone's more menu, a reader who may not fork).
- `commentMode(on: boolean)` enters or leaves comment mode
  (`ArtifactController.toggleComment`), from a click handler only
  (`no_gesture` otherwise).
- `can(control)` resolves whether that control is shown.

It reveals nothing about the shell beyond whether a control exists, and
nothing about agents or harnesses (D8).

## 9. G. The quickstart in every new gallery

- `web/quickstart/` holds the page (`index.html`, `ideas.html`,
  `style.css`, `ideas-data.js`, `icon.svg`, `clax.json`). The web build
  copies it into `web/dist/quickstart/`, which `clax-embedded` already
  embeds.
- **Local.** When the owner's gallery is first listed and the home has
  never seeded the quickstart (a `home_flags` row `quickstart_seeded`),
  the daemon publishes the bundled quickstart as a new artifact owned by
  the owner, pinned, tagged with its release (`quickstart@<release>`), and
  sets the flag. The empty gallery is never seen empty.
- **Hosted.** A person new through GitHub sign-in (`GitHubOutcome::New`)
  or an invite gets the same, owned by them.
- **Updates.** At start, a seeded quickstart whose current version is
  still the one Clax published (no other version since) gets the bundled
  release's newer files as a new version, labelled "Quickstart
  <release>". Once the person publishes or forks a change, Clax stops.
- Deleting it is final: the flag stays set.

## 10. H. Always publish to clax.dev

Not in this branch: the owner suggested it ("maybe"), and its idea is
still `proposed`. Kept here so the design is ready if it is accepted.

- `config.toml` `[publish]`: `remote` (an origin; default none) and
  `always` (default `false`). With both set and this Clax signed in
  there, a new artifact's first version is pushed with `auto` (follow), as
  `clax push --follow` does, from the same spawned path as
  `public::after_publish`.
- A page opts out with Unlink in its Publish dialog; it is not pushed
  again.
- Pages go up Restricted (only their owner) until shared.
- An agent's `publish_public` does not ask for a page this setting
  covers.

## 11. Build order

A (done) → B → C → D → E → F → G (H when accepted). B before E (E reuses B's flow); C
before D (approvals are of declarations per version); D before E and F
(their consents are D's grants); G last but one so the shipped quickstart
uses E and F.

## 12. Open questions for the owner

- Q1. E hosted: a world visitor's page runs on the site domain, not the
  public origin. After `/auth/github` returns, does the site see the new
  session at once, or does it need its own hand-off? To check against
  `crates/clax-worker` and `site_access` before building §5.4.
- Q2. E local: may a page sign in to an origin other than clax.dev when
  it declares no `origins`?
- Q3. E: should `status()` be available without declaring `account`?
- Q4. D: should the owner's own and their agents' first versions really
  skip approval (§7), or should every artifact ask once?
- Q5. G: should the seeded quickstart keep following releases after the
  person comments on it (comments are not changes), and stop only on a
  new version they or their agent publish (§9)?
- Q6. H: `always` pushes every new artifact, drafts excluded. Should it
  also push existing artifacts when first turned on?
