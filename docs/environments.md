# Environments: prod, dev and previews

Hosted Clax runs in three kinds of environment, each a Cloudflare Worker
with resources of its own, deployed by the same script
(`scripts/cloudflare-setup.sh`, which runs `crates/clax-worker/deploy.mjs`)
from GitHub Actions:

| env | Worker | Hosts | Versions | Deployed by | GitHub environment |
|---|---|---|---|---|---|
| `prod` | `clax` | `clax.dev`, `<ID>.clax.run` | Artifacts (bucket `clax-assets`, queue `clax-artifacts-events`, namespace `clax`) | every push to `prod` (`deploy-prod.yml`, [docs/prod.md](prod.md)) | `production` |
| `dev` | `clax-dev` | `PUBLIC_HOST`, `<ID>.SITE_DOMAIN` | Artifacts (bucket `clax-assets-dev`, queue `clax-artifacts-events-dev`, namespace `clax-dev`) | every push to `main` whose `ci` run passes (`deploy-dev.yml`) | `dev` |
| `pr-<N>` | `clax-pr-<N>` | `pr-<N>.PREVIEW_HOST_ZONE`, `<ID>.pr-<N>.PREVIEW_SITE_ZONE` | memory (the RepoDO stand-in), assets in the shared bucket `clax-assets-preview` | every push to a pull request an organization member opens from a branch of this repository (`preview.yml`) | `preview` |

The `clax-dev` Worker is the dev environment, not clax.dev; clax.dev is
`prod`'s Worker `clax` (and `just clax-dev-deploy` deploys it).

Every environment has GitHub sign-in, through a GitHub OAuth App of its
own, and `OWNER_GITHUB` signs in as its owner.

## How it fits together

`deploy.mjs --env <name>` (and `cloudflare-setup.sh --env <name>`, which
passes it on) picks the environment's names from one mapping,
`envOf` in `crates/clax-worker/deploy-lib.mjs`, which `teardown.mjs` reads
too, so a deploy and a teardown cannot disagree about which resources a
name refers to:

- **prod** keeps clax.dev's names, so its deploys are as they were.
- **dev** has a bucket, queue and Artifacts namespace of its own. Sharing
  prod's would mix the two: a queue takes one consumer Worker, and neither
  an asset's key (`assets/<artifact ID>/…`) nor a repository's name says
  which instance it belongs to.
- **A preview** (`pr-<N>`, or any lowercase name of at most 40 characters)
  keeps its versions in memory, so it needs no queue, namespace or event
  subscription per pull request. Its assets go to the one bucket every
  preview shares, `clax-assets-preview`, whose objects expire 30 days after
  they are written (an R2 lifecycle rule `deploy.mjs` sets), so a torn-down
  preview's assets do not outlive it by more than that.

Each environment keeps its laptop settings in a file of its own beside
prod's `setup.env`: `~/.config/clax/deploy/setup.<name>.env`. The deploy
record is `<public host>.json` there, as for prod.

### A preview's hosts

A preview's site domain is `pr-<N>.PREVIEW_SITE_ZONE`, one level below the
zone, so its artifacts' hosts (`<ID>.pr-<N>.PREVIEW_SITE_ZONE`) are two
levels below it. The deploy, given `--site-zone PREVIEW_SITE_ZONE`:

- puts the Worker's route `*.pr-<N>.PREVIEW_SITE_ZONE/*` on that zone;
- makes the proxied DNS record `*.pr-<N>.PREVIEW_SITE_ZONE` (AAAA `100::`)
  through the Cloudflare API (`crates/clax-worker/cloudflare-api.mjs`);
- relies on **Total TLS** on the zone for the certificate: Universal SSL
  covers one level below the zone only. Total TLS issues a certificate for
  each proxied host, the wildcard record's included, and needs Advanced
  Certificate Manager.

The public host `pr-<N>.PREVIEW_HOST_ZONE` is a Workers custom domain, for
which Cloudflare makes the DNS record and the certificate itself.

`PREVIEW_HOST_ZONE` and `PREVIEW_SITE_ZONE` are two registrable domains of
their own (the same rule as prod's `clax.dev` and `clax.run`: a site must
never be the same site, to a browser, as Clax).

### A preview's lifecycle

- **First push:** a fresh instance. CI gives it no `SETUP_SECRET` (setup
  stays closed) and no `CLAX_TOKEN` (the instance makes its own); it
  generates its `CLAX_KEK` and keeps no backup, since the preview's data is
  thrown away with it. The pull request gets a comment with its address.
  Sign in with GitHub as `OWNER_GITHUB` to be its owner; anyone else who
  signs in with GitHub is a new person, as on clax.dev.
- **Later pushes:** the same Worker, redeployed: its artifacts and people
  stay (they live in its Durable Objects).
- **Close or merge:** `preview.yml`'s teardown job runs
  `teardown.mjs pr-<N>`, which deletes the route, the DNS record, the
  custom domain and the Worker with its Durable Objects.
- **Missed closes:** GitHub sends no `closed` event when it cannot build a
  pull request's merge ref. `preview-reaper.yml` runs daily (and by hand,
  with a dry run) `teardown.mjs --reap`, which tears down every
  `clax-pr-<N>` Worker whose pull request is closed or gone.

A preview runs no smoke: the deploy's health wait and site host check
fail its run. It does not exercise the Artifacts store, its queue or the
diagnostics; dev does, on every merge to `main`.

Only Empathic's people get previews: `preview.yml` deploys a pull request
only when its author is an owner or member of the organization that owns
this repository (the event's `author_association`); anyone else's shows
the deploy job as skipped. Pull requests from forks get no preview either:
their runs are not given the `preview` environment's secrets.

## GitHub sign-in on every environment

Each environment has its own GitHub OAuth App (GitHub, Settings,
Developer settings, OAuth Apps), created by hand, with the scope Clax asks
for (`read:user`) and no other setting beyond these:

| App | Homepage | Callback URL | Wildcard matching |
|---|---|---|---|
| dev | `https://PUBLIC_HOST` | `https://PUBLIC_HOST/auth/github/callback` | off |
| preview | `https://PREVIEW_HOST_ZONE` | `https://PREVIEW_HOST_ZONE/auth/github/callback` | **on** |

Clax sends `redirect_uri=<public origin>/auth/github/callback`
(`crates/clax-server/src/github.rs`). With wildcard matching on, GitHub
takes any subdomain of the callback URL's host with the same port and a
path at or under its path, so the one preview App serves every
`https://pr-<N>.PREVIEW_HOST_ZONE`
([GitHub's docs](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps),
"Redirect URLs"). The App's client ID is the environment's variable
`OAUTH_CLIENT_ID` (GitHub refuses variable names starting with `GITHUB_`)
and its client secret the environment's secret `OAUTH_CLIENT_SECRET`, which
the workflows pass to the deploy as `GITHUB_CLIENT_SECRET`.

## Credentials

A Cloudflare API token's Workers permissions are account-wide: one that
can deploy `clax-pr-12` can also replace `clax`. A preview's run executes
the pull request's own scripts with the `preview` environment's token, so
anyone who can push a branch to this repository can use that token.

**dev and the previews share clax.dev's Cloudflare account, for now.** So
the dev and preview tokens can reach prod's Worker `clax`, its Durable
Objects and its bucket `clax-assets`: a branch pushed to this repository
can replace or delete them through its preview's run. Pull requests from
forks get no preview, so no token reaches them.

Deploying only organization members' pull requests narrows that, but does
not bound it: a pull request's run takes `preview.yml` from the pull
request itself, so anyone who can push a branch here can open a pull
request whose copy drops the check, or push to a member's pull request.
What bounds who reaches the token is who has write access to this
repository: keep it to Empathic's people. Moving dev and the
previews to an account of their own takes that reach away: each
environment names its account in its `CLOUDFLARE_ACCOUNT_ID` variable, so
the move is new tokens, zones and variables, and no code change.

Give each environment its own token, limited to the account and to its
zones (never clax.dev's or clax.run's):

| Scope | Permission | dev | preview |
|---|---|---|---|
| Account | Workers Scripts: Edit | yes | yes (deploy, and teardown's deletes) |
| Account | Workers R2 Storage: Edit | yes | yes (the bucket and its lifecycle rule) |
| Account | Queues: Edit | yes | no |
| Account | Artifacts: Edit | yes | no |
| Account | Account Settings: Read | yes | yes |
| Zone (public host's) | Zone: Read | yes | yes |
| Zone (site domain's) | Workers Routes: Edit, Zone: Read | yes | yes |
| Zone (site domain's) | DNS: Edit | when `SITE_ZONE` is set | yes (the `*.pr-<N>` records) |
| User | User Details: Read, Memberships: Read | yes | yes |

The same caveats as prod's token apply ([docs/prod.md](prod.md), "The
Cloudflare API token"): some permission names are not confirmed by
Cloudflare's docs.

## One-time setup

### Cloudflare

1. On the account the environments use, add the zones: dev's public
   host's and site domain's, and `PREVIEW_HOST_ZONE` and
   `PREVIEW_SITE_ZONE`.
2. Turn on Advanced Certificate Manager and Total TLS on
   `PREVIEW_SITE_ZONE` (and on dev's site zone when dev's site domain is
   below its zone, `SITE_ZONE`). A dev site domain that is a zone itself
   needs its proxied `*` record and a certificate covering it, as prod's
   does (`crates/clax-worker/README.md`, "DNS for the public host and the
   site domain").
3. Make the two tokens above.

### dev

1. Create dev's OAuth App (above).
2. Deploy dev once from a laptop, which makes its secrets, as prod's first
   deploy did:

   ```
   GITHUB_CLIENT_SECRET=… just env-deploy dev --public <host> --site-domain <domain> \
     --github-client-id <ID> --owner-github <login> --account <ID> --i-know-what-i-am-doing
   ```

   The settings are kept in `~/.config/clax/deploy/setup.dev.env`, and
   later runs need no flags.
3. Sign in to dev as the owner, make an agent token named `smoke`
   (settings, Agent tokens).
4. Set up the `dev` GitHub environment (deployable from `main` only) and
   its variables from that configuration, then set its three secrets as
   the script prints them:

   ```
   scripts/setup-env-github.sh --env dev                            # dry run
   scripts/setup-env-github.sh --env dev --i-know-what-i-am-doing   # act
   ```

Until `CLAX_SMOKE_TOKEN` is set, `deploy-dev.yml` deploys without the smoke
and warns.

### Previews

1. Create the preview OAuth App (above), wildcard matching on.
2. Set up the `preview` GitHub environment (deployable from any branch)
   and its variables, then its two secrets as the script prints them:

   ```
   scripts/setup-env-github.sh --env preview --preview-host-zone <zone> --preview-site-zone <zone> \
     --github-client-id <ID> --owner-github <login> --cloudflare-account-id <ID>             # dry run
   … --i-know-what-i-am-doing                                                               # act
   ```

The next push to a pull request deploys its preview.

## By hand

```
just env-deploy pr-7 --public pr-7.<zone> --site-domain pr-7.<site zone> --site-zone <site zone> …   # a dry run
just env-teardown pr-7 --site-zone <site zone> --dry-run                                            # CLOUDFLARE_API_TOKEN, CLOUDFLARE_ACCOUNT_ID
```

`just env-deploy` refuses `prod` (that is `just clax-dev-deploy`), and
`teardown.mjs` refuses `dev` and `prod`.
