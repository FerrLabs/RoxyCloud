# RoxyCloud

Self-hosted file storage in Rust. Web UI, REST API, and WebDAV over a content-addressed blob store.

A [FerrLabs](https://github.com/FerrLabs) project, licensed [AGPL-3.0](LICENSE).

See [ARCHITECTURE.md](ARCHITECTURE.md) for the design.

## Status

Early, and not yet usable end to end.

Done: content-addressed local blob store with dedup, the node tree with quotas and blob refcounts,
and upload, download, listing and trash over REST.

Done too: password accounts with Argon2id, session tokens, and login from the web app, the desktop
shell and the CLI, plus the marketing and documentation site in English and French.

And: folder sync, with a three-way reconciler that keeps both copies when a file changed on either
side, either once or watching the folder as it changes.

Not written: app passwords, WebDAV, sharing, search, OIDC, the S3 backend, the orphan blob sweeper,
and any interface for the sync beyond the command line.

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
| `CORS_ALLOWED_ORIGINS` | empty | Comma-separated origins for the SPA |
| `DEFAULT_QUOTA_BYTES` | 10 GiB | Quota granted on first write |
| `SESSION_TTL_SECONDS` | 12 h | Session token lifetime |
| `BOOTSTRAP_ADMIN_EMAIL` | unset | Creates the first administrator on an empty database |
| `BOOTSTRAP_ADMIN_PASSWORD` | unset | Required alongside the email, minimum 12 characters |

The web app compiles two values in, `ROXYCLOUD_API_URL` and `ROXYCLOUD_SOURCE_URL`. They default to
a local API and to this repository, and both are overridden at build time:

```bash
pnpm --filter @roxycloud/web build   --define ROXYCLOUD_API_URL="'https://api.example.com'"   --define ROXYCLOUD_SOURCE_URL="'https://git.example.com/roxycloud'"
```

If you deploy a modified RoxyCloud, point the source URL at your fork: the AGPL requires you to
offer your users the source of the version they are actually using.

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
```

Every `/v1` route except login takes `Authorization: Bearer <session token>`.

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

Two things it deliberately does not do. An empty local directory is not created on the server, since
there is no endpoint for that yet, and a directory removed locally is not removed on the server,
because the API trashes a directory node without cascading to its children.

## Development

```bash
pnpm install && pnpm run build
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

`pnpm run build` builds both browser surfaces. `web/dist` is embedded in the desktop build, so
build the web app before touching `app/`. On Linux the Tauri crate needs `libwebkit2gtk-4.1-dev`,
`libappindicator3-dev`, `librsvg2-dev` and `patchelf`.

The site is a separate Angular app under `site/`, prerendered to static files in `site/dist`, with
`pnpm run dev:site` for the dev server. It carries the install and API pages, so a change to a
config key or an endpoint updates `site/src/app/content/` in the same pull request.

See [CONTRIBUTING.md](CONTRIBUTING.md).
