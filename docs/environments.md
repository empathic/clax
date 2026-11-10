# Environments: prod, dev and previews

Hosted Clax runs in three kinds of environment, each a Cloudflare Worker
with resources of its own, deployed by the same script
(`scripts/cloudflare-setup.sh`, which runs `crates/clax-worker/deploy.mjs`)
from GitHub Actions:

| env | Worker | Hosts | Versions | Deployed by | GitHub environment |
|---|---|---|---|---|---|
| `prod` | `clax` | `clax.dev`, `<ID>.clax.run` | Artifacts (bucket `clax-assets`, queue `clax-artifacts-events`, namespace `clax`) | every push to `prod` (`deploy-prod.yml`, [docs/prod.md](prod.md)) | `production` |
| `dev` | `clax-dev` | `dev.PREVIEW_HOST_ZONE`, `<ID>.dev.PREVIEW_SITE_ZONE` | Artifacts (bucket `clax-assets-dev`, queue `clax-artifacts-events-dev`, namespace `clax-dev`) | every push to `main` whose `ci` run passes (`deploy-dev.yml`) | `preview` |
| `pr-<N>` | `clax-pr-<N>` | `pr-<N>.PREVIEW_HOST_ZONE`, `<ID>.pr-<N>.PREVIEW_SITE_ZONE` | Artifacts (queue `clax-artifacts-events-pr-<N>`, namespace `clax-pr-<N>`), assets in the shared bucket `clax-assets-preview` | every push to a pull request from a branch of this repository, except a release pull request into `prod` (`preview.yml`) | `preview` |
| staging, `pr-<N>` | `clax-pr-<N>`, on prod's account | `pr-<N>.clax-staging.dev`, `<ID>.pr-<N>.clax-staging.run` | as a preview's, on prod's account | every push to a release branch whose pull request into `prod` is open (`staging.yml`, [docs/prod.md](prod.md), "Staging") | `staging` |

The `clax-dev` Worker is the dev environment, not clax.dev; clax.dev is
`prod`'s Worker `clax` (and `just clax-dev-deploy` deploys it).

Every environment has GitHub sign-in, and `OWNER_GITHUB` signs in as its
owner: prod through its own GitHub OAuth App, dev and the previews through
the preview one.

dev runs in the `preview` GitHub environment: dev and the previews need
the same Cloudflare account, token and OAuth App, and dev's hosts are
named from the preview zones. `deploy-dev.yml` deploys from `main` alone
(its job's condition); the environment itself admits any branch, for the
previews.

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
  keeps its versions on Artifacts, as dev and prod do, in a queue
  (`clax-artifacts-events-pr-<N>`) and namespace (`clax-pr-<N>`) of its
  own, made by its first deploy and deleted by its teardown. Its assets go
  to the one bucket every preview shares, `clax-assets-preview`, whose
  objects expire 30 days after they are written (an R2 lifecycle rule
  `deploy.mjs` sets), so a torn-down preview's assets do not outlive it by
  more than that. (`--store memory` still deploys one on the RepoDO
  stand-in, by hand.)

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
  generates its `CLAX_KEK` and keeps no copy, since the preview's data is
  thrown away with it (dev's and prod's are backed up by their operator,
  [docs/prod.md](prod.md), "CLAX_KEK"). The pull request gets a comment with its address.
  Sign in with GitHub as `OWNER_GITHUB` to be its owner; anyone else who
  signs in with GitHub is a new person, as on clax.dev.
- **Later pushes:** the same Worker, redeployed: its artifacts and people
  stay (they live in its Durable Objects).
- **Close or merge:** `preview.yml`'s teardown job runs
  `teardown.mjs pr-<N>`, which deletes the route, the DNS record, the
  custom domain, the Worker with its Durable Objects, then its queue and
  every repository in its namespace. The namespace itself stays, empty:
  Cloudflare's API deletes repositories, not namespaces.
- **Missed closes:** GitHub sends no `closed` event when it cannot build a
  pull request's merge ref. `preview-reaper.yml` runs daily (and by hand,
  with a dry run) `teardown.mjs --reap`, which tears down every preview
  whose pull request is closed or gone: each `clax-pr-<N>` Worker, and
  each `clax-artifacts-events-pr-<N>` queue a teardown left behind.

A preview runs no smoke: the deploy's health wait and site host check
fail its run. It runs on the Artifacts store, but nothing checks its queue
path or the diagnostics; dev's smoke does, on every merge to `main`.

Only writers' branches get previews: `preview.yml` deploys a pull request
whose branch is in this repository, which only someone with write access
can push. Pull requests from forks get no preview: their runs are not
given the `preview` environment's secrets.

## GitHub sign-in on every environment

dev and the previews share one GitHub OAuth App (GitHub, Settings,
Developer settings, OAuth Apps), created by hand, with the scope Clax asks
for (`read:user`) and no other setting beyond these:

| App | Homepage | Callback URL | Wildcard matching |
|---|---|---|---|
| preview | `https://PREVIEW_HOST_ZONE` | `https://PREVIEW_HOST_ZONE/auth/github/callback` | **on** |

Clax sends `redirect_uri=<public origin>/auth/github/callback`
(`crates/clax-server/src/github.rs`). With wildcard matching on, GitHub
takes any subdomain of the callback URL's host with the same port and a
path at or under its path, so the one App serves every
`https://pr-<N>.PREVIEW_HOST_ZONE` and `https://dev.PREVIEW_HOST_ZONE`
([GitHub's docs](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps),
"Redirect URLs"). The App's client ID is the environment's secret
`OAUTH_CLIENT_ID` (GitHub refuses secret names starting with `GITHUB_`)
and its client secret the environment's secret `OAUTH_CLIENT_SECRET`, which
the workflows pass to the deploy as `GITHUB_CLIENT_SECRET`.

## Credentials

A Cloudflare API token's Workers permissions are account-wide: one that
can deploy `clax-pr-12` can also replace `clax`. A preview's run executes
the pull request's own scripts with the `preview` environment's token, so
anyone who can push a branch to this repository can use that token.

Who can use the preview token is who has write access to this repository:
only a writer can push the branch a preview runs, and pull requests from
forks get no preview (their runs are not given the environment's
secrets). A check on the pull request's author in `preview.yml` would
bound nothing more, since a run takes `preview.yml` from the pull request
itself.

**So dev and the previews run on a Cloudflare account of their own**, with
their zones on it, apart from clax.dev's: neither token can reach prod's
Worker `clax`, its Durable Objects or its bucket `clax-assets`, whatever a
branch's run does with it. The `preview` environment names that account
in its secret `CLOUDFLARE_ACCOUNT_ID`. The account needs Workers Paid
(Durable Objects and the Worker Loader) and the Artifacts beta.

dev and the previews need the same permissions, so one token, limited to
that account and to its zones, serves both, in the `preview` environment.
So does dev's smoke token, `CLAX_DEV_SMOKE_TOKEN`: a writer's branch can
act as an agent on dev, and redeploy over `clax-dev`, as it could with
the Cloudflare token alone.

| Scope | Permission | For |
|---|---|---|
| Account | Workers Scripts: Edit | deploys, and teardown's deletes |
| Account | Workers R2 Storage: Edit | the buckets, and the preview bucket's lifecycle rule |
| Account | Queues: Edit | each environment's queue, its consumer, and teardown's deletes |
| Account | Artifacts: Edit | each environment's namespace, and teardown's repository deletes |
| Account | Account Settings: Read | the login check |
| Zone (public host's) | Zone: Read | the custom domain |
| Zone (site domain's) | Workers Routes: Edit, Zone: Read | the site route |
| Zone (site domain's) | DNS: Edit | the `*.pr-<N>` and `*.dev` records |
| User | User Details: Read, Memberships: Read | the login check |

The same caveats as prod's token apply ([docs/prod.md](prod.md), "The
Cloudflare API token"): some permission names are not confirmed by
Cloudflare's docs.

## One-time setup

### Cloudflare

1. Make the account dev and the previews use (not clax.dev's), on Workers
   Paid, with R2 turned on (Storage & databases, R2; until it is, every
   deploy fails making its bucket with Cloudflare's code 10042) and the
   Artifacts beta turned on (Storage & databases, Artifacts).
   `just wrangler login`, then `just wrangler whoami` lists the accounts the login sees, with their IDs: the new one's is
   the `preview` environment's `CLOUDFLARE_ACCOUNT_ID`.
2. On it, add the zones `PREVIEW_HOST_ZONE` and `PREVIEW_SITE_ZONE`.
3. Turn on Advanced Certificate Manager and Total TLS on
   `PREVIEW_SITE_ZONE`: the sites `<ID>.pr-<N>.PREVIEW_SITE_ZONE` and
   `<ID>.dev.PREVIEW_SITE_ZONE` are two levels below it, past what
   Universal SSL covers.
4. Make the token above.

### Previews

1. Create the preview OAuth App (above), wildcard matching on.
2. Set up the `preview` GitHub environment (deployable from any branch)
   and its variables, then its secrets as the script prints them (dev's
   smoke token waits for dev, below):

   ```
   scripts/setup-env-github.sh --env preview --preview-host-zone <zone> --preview-site-zone <zone> \
     --github-client-id <ID> --owner-github <login> --cloudflare-account-id <ID>             # dry run
   … --i-know-what-i-am-doing                                                               # act
   ```

The next push to a pull request deploys its preview.

### dev

1. Make dev's `CLAX_KEK` (`openssl rand -hex 32`) where its backup is
   kept, its only copy outside the Worker ([docs/prod.md](prod.md),
   "CLAX_KEK").
2. Deploy dev once from a laptop, which makes its secrets and uploads that
   key, as prod's first deploy did, with the preview OAuth App's client
   secret:

   ```
   read -rs CLAX_KEK && export CLAX_KEK   # paste dev's key
   GITHUB_CLIENT_SECRET=… just env-deploy dev --public dev.<PREVIEW_HOST_ZONE> \
     --site-domain dev.<PREVIEW_SITE_ZONE> --site-zone <PREVIEW_SITE_ZONE> \
     --github-client-id <ID> --owner-github <login> --account <ID> --i-know-what-i-am-doing
   ```

   The settings are kept in `~/.config/clax/deploy/setup.dev.env`, and
   later runs need no flags.
3. Sign in to dev as the owner, make an agent token named `smoke`
   (Settings, Agents & machines, Agent tokens), and set it as the `preview` environment's
   `CLAX_DEV_SMOKE_TOKEN`.

Every green push to `main` deploys dev from then on. Until
`CLAX_DEV_SMOKE_TOKEN` is set, `deploy-dev.yml` deploys without the smoke
and warns.

## By hand

```
just env-deploy pr-7 --public pr-7.<zone> --site-domain pr-7.<site zone> --site-zone <site zone> …   # a dry run
just env-teardown pr-7 --site-zone <site zone> --dry-run                                            # CLOUDFLARE_API_TOKEN, CLOUDFLARE_ACCOUNT_ID
```

`just env-deploy` refuses `prod` (that is `just clax-dev-deploy`), and
`teardown.mjs` refuses `dev` and `prod`.
