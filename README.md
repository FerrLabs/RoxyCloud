<div align="center">

<img src="site/public/favicon.svg" alt="" width="88" height="88" />

# RoxyCloud

**Self-hosted file storage in Rust.**

A web app, a REST API and folder sync over a content-addressed blob store.<br />
Your files, on hardware you own, under the AGPL.

[![Latest release](https://img.shields.io/github/v/release/FerrLabs/RoxyCloud)](https://github.com/FerrLabs/RoxyCloud/releases/latest)
[![CI](https://github.com/FerrLabs/RoxyCloud/actions/workflows/ci.yml/badge.svg)](https://github.com/FerrLabs/RoxyCloud/actions/workflows/ci.yml)
[![Conventional Commits](https://img.shields.io/badge/Conventional%20Commits-1.0.0-%23FE5196?logo=conventionalcommits&logoColor=white)](https://www.conventionalcommits.org/en/v1.0.0/)
[![License](https://img.shields.io/github/license/FerrLabs/RoxyCloud)](LICENSE)

[Architecture](ARCHITECTURE.md) | [Contributing](CONTRIBUTING.md) | [Security](SECURITY.md) | [FerrLabs](https://github.com/FerrLabs)

</div>

## Status

Early, and not yet usable end to end.

Done: content-addressed local blob store with dedup, the node tree with quotas and blob refcounts,
and upload, download, listing, trash and restore over REST.

Done too: password accounts with Argon2id, session tokens, and login from the web app, the desktop
shell and the CLI, plus the marketing and documentation site in English and French. The file browser
uploads, previews, renames and deletes, and renaming doubles as moving: type a path instead of a name
and the node lands there.

And: folder sync, with a three-way reconciler that keeps both copies when a file changed on either
side, either once or watching the folder as it changes.

Not written: sharing, search, OIDC, the S3 backend, WebDAV locking, and any interface for the sync
beyond the command line. Without locking the surface advertises class 1, which macOS Finder and
Windows Explorer read as read-only.

## Layout

```
api/        Rust: the server, the domain, the migrations
web/        Angular app, shared by the browser and the desktop shell
site/       Angular marketing and documentation site, prerendered
deploy/     Dockerfile, compose file, Helm chart
```

## Running the API

Postgres 15 or later, and a Rust toolchain matching `rust-toolchain.toml`.

```bash
DATABASE_URL=postgres://localhost/roxycloud JWT_SECRET=dev-secret cargo run -p roxycloud-api
```

Migrations run on boot. Configuration is environment only:

| Variable | Default | Purpose |
|---|---|---|
| `DATABASE_URL` | required | Postgres connection string |
| `JWT_SECRET` | required | HS256 secret used to sign session tokens |
| `PORT` | `3001` | Listen port |
| `BLOB_ROOT` | `./data` | Local blob store root |
| `WEB_ROOT` | unset | Directory holding the built web app, served alongside the API |
| `CORS_ALLOWED_ORIGINS` | empty | Comma-separated origins for the SPA, not needed when `WEB_ROOT` serves it |
| `DEFAULT_QUOTA_BYTES` | 10 GiB | Quota granted on first write |
| `SESSION_TTL_SECONDS` | 12 h | Session token lifetime |
| `BLOB_SWEEP_INTERVAL_SECONDS` | 1 h | How often orphaned blobs are collected, `0` disables it |
| `BLOB_GRACE_PERIOD_SECONDS` | 24 h | How long an unreferenced blob is kept before collection |
| `BOOTSTRAP_ADMIN_EMAIL` | unset | Creates the first administrator on an empty database |
| `BOOTSTRAP_ADMIN_PASSWORD` | unset | Required alongside the email, minimum 12 characters |

The web app compiles two values in, `ROXYCLOUD_API_URL` and `ROXYCLOUD_SOURCE_URL`. They default to
a local API and to this repository, and both are overridden at build time. An empty API URL means
the same origin as the page, which is what the image builds with, since the API serving the app is
also the API it talks to:

```bash
pnpm --filter @roxycloud/web build   --define ROXYCLOUD_API_URL="'https://api.example.com'"   --define ROXYCLOUD_SOURCE_URL="'https://git.example.com/roxycloud'"
```

If you deploy a modified RoxyCloud, point the source URL at your fork: the AGPL requires you to
offer your users the source of the version they are actually using.

## Self-hosting

`deploy/docker-compose.yml` brings up the API, the web app and a Postgres for them, on
`http://localhost:3001`:

```bash
POSTGRES_PASSWORD=... JWT_SECRET=... docker compose -f deploy/docker-compose.yml up -d --build
```

The image carries the built web app and serves it from the same origin as the API, so there is no
second deployment and no CORS to configure. Hosting the bundle elsewhere still works: build it with
`ROXYCLOUD_API_URL` pointing at the API, serve it however you like, and name its origin in
`CORS_ALLOWED_ORIGINS`.

On Kubernetes, `deploy/helm/roxycloud` deploys the API against a database you already run, with a
volume for the blobs and an optional ingress. It does not bundle Postgres. It serves the web app,
since the image carries it. `deploy/helm/roxycloud/README.md` has the values and the reasoning.

```bash
helm install roxycloud deploy/helm/roxycloud   --set database.url='postgres://roxycloud:password@postgres/roxycloud'   --set jwt.secret="$(openssl rand -hex 32)"
```

The image is `ghcr.io/ferrlabs/roxycloud-api`, published for amd64 and arm64 by the release
workflow, so the chart's default needs no override.

## Endpoints

```
GET    /health
POST   /v1/auth/login       exchange email and password for a session token
GET    /v1/auth/me          the authenticated account
GET    /v1/folders            list the root
GET    /v1/folders/{*path}    list a directory
PUT    /v1/files/{*path}      upload, creating parent directories
GET    /v1/files/{*path}      download
DELETE /v1/files/{*path}      move to trash
POST   /v1/move               rename a node, or move it under another directory
GET    /v1/app-passwords      the credentials this account has minted
POST   /v1/app-passwords      mint one, shown once
DELETE /v1/app-passwords/{id} revoke one, taking effect immediately

OPTIONS   /dav          what the WebDAV surface supports
PROPFIND  /dav/{*path}  list a collection, Depth 0 or 1
PROPPATCH /dav/{*path}  answered, and refused: no dead properties are stored
MKCOL     /dav/{*path}  create a collection
GET       /dav/{*path}  download
PUT       /dav/{*path}  upload, without inventing the collections above it
DELETE    /dav/{*path}  move to trash
COPY      /dav/{*path}  copy, sharing the bytes rather than writing them again
MOVE      /dav/{*path}  move, in one transaction
GET    /v1/trash              what the account has deleted
POST   /v1/trash/{id}/restore bring it back, with the directories it needs
DELETE /v1/trash/{id}         delete it for good, and release its bytes
```

Every `/v1` route except login takes `Authorization: Bearer <session token>`.

Deleting is reversible. `DELETE /v1/files/{*path}` marks the node and everything under it, credits
the quota and leaves the bytes alone, so `GET /v1/trash` lists what was deleted and a restore puts it
back where it was, recreating any directory above it that was deleted in the meantime. A name taken
since the delete answers 409 rather than inventing a new one: move the occupant, then restore. What
was deleted separately stays separate, so restoring a file out of a folder someone deleted later
leaves the rest of that folder in the trash, listed on its own. Only a purge releases the blobs,
which is what makes it the one irreversible call, and purging a folder takes everything trashed
under it, including what was deleted before it.

Releasing a blob does not delete it. A background sweep collects blobs nothing points at once they
have been unreferenced for `BLOB_GRACE_PERIOD_SECONDS`, which is what keeps a delete followed by a
re-upload of the same content from racing the collector: the re-upload finds the blob and adopts it.
The bytes come back to the disk on that schedule, not on the purge.

A WebDAV client stores its credential in plain text more often than not, so it never gets the
account password. `POST /v1/app-passwords` mints a high-entropy secret, shows it once, and keeps only
a fingerprint of it. The secret authenticates over Basic auth on the WebDAV surface and nowhere else:
account management answers 401 to it, so a stolen credential cannot mint itself a successor. Revoking
takes effect on the next request, and `last_used_at` says which credentials nothing is using.

`/dav` speaks WebDAV against the same tree, authenticated by Basic auth with an app password and
nothing else: a session token is answered 401 there. It advertises class 1, so clients that require
class 2 for writes will not write until locking lands. A `COPY` shares the blob rather than storing
the bytes twice, and quota is charged for the copy because the tree grew.

Each account carries a role: `admin`, `member` or `reader`. A reader may list and download; upload
and delete answer 403. The check sits in the API rather than in the interface, so it holds for curl
and for `roxy sync` as much as for the web app. There is no route that creates an account or changes
a role yet, so the only way to make one today is the bootstrap administrator or SQL.

On an empty database, set `BOOTSTRAP_ADMIN_EMAIL` and `BOOTSTRAP_ADMIN_PASSWORD` for the first boot
to create the administrator, then log in:

```bash
cargo run -p roxycloud-cli -- login you@example.com --password '...'
```

## Syncing a folder

`roxy sync` reconciles a local folder with the server once and prints what it did. It compares
content, not timestamps: a file is only transferred when its bytes differ from the other side.

```bash
ROXYCLOUD_TOKEN=... cargo run -p roxycloud-cli -- sync ~/RoxyCloud
```

State lives in `.roxycloud-sync.json` inside the folder, which is what makes a second run cheap and
what tells a deletion apart from a file that was never there. Delete it to start from a full
comparison again.

When a file changed on both sides, both copies are kept: the server's version keeps the name, and
the local one is renamed `name (conflict <timestamp>).ext` and uploaded under that name. Nothing is
overwritten and nothing waits for an answer.

`--watch` keeps it running instead, syncing as the folder changes:

```bash
ROXYCLOUD_TOKEN=... cargo run -p roxycloud-cli -- sync ~/RoxyCloud --watch
```

A save is not a sync. Changes are collected until the folder has been quiet for a moment, and a
folder that never goes quiet still syncs at a ceiling rather than waiting forever. Editors that
write a temp file, rename it, and touch the directory therefore produce one sync, not four. Ctrl+C
stops it.

One thing it deliberately does not do: an empty local directory is not created on the server, since
there is no endpoint for that yet.

Removing a folder locally removes it on the server, contents first and the folder itself last. It
holds back when the server's copy has gained anything the last sync did not see, a file added from
another machine or an edit to one that is already there, because the delete would take that with it.
The folder stays, the new work comes down, and the next removal is the user's to make with both
sides in front of them.

## Development

```bash
pnpm install && pnpm run build
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

The tests that need Postgres skip themselves when `DATABASE_URL` is unset, so the line above runs
anywhere. Point it at a database and they run:

```bash
DATABASE_URL=postgres://roxy:roxy@localhost:5432/roxycloud cargo test --workspace
```

`pnpm run build` builds both browser surfaces. `web/dist` is embedded in the desktop build, so
build the web app before touching `app/`. On Linux the Tauri crate needs `libwebkit2gtk-4.1-dev`,
`libappindicator3-dev`, `librsvg2-dev` and `patchelf`.

The site is a separate Angular app under `site/`, prerendered to static files in `site/dist`, with
`pnpm run dev:site` for the dev server. It carries the install and API pages, so a change to a
config key or an endpoint updates `site/src/app/content/` in the same pull request.

See [CONTRIBUTING.md](CONTRIBUTING.md).
