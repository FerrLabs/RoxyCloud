# RoxyCloud

Self-hosted file storage in Rust. Web UI, REST API, and WebDAV over a content-addressed blob store.

A [FerrLabs](https://github.com/FerrLabs) project, licensed [AGPL-3.0](LICENSE).

See [ARCHITECTURE.md](ARCHITECTURE.md) for the design.

## Status

Early, and not yet usable end to end.

Done: content-addressed local blob store with dedup, the node tree with quotas and blob refcounts,
and upload, download, listing and trash over REST.

Done too: password accounts with Argon2id, session tokens, and login from the web app, the desktop
shell and the CLI.

Not written: app passwords, WebDAV, sharing, search, OIDC, the S3 backend, the orphan blob sweeper,
the sync engine, and the marketing site.

## Layout

```
api/        Rust: the server, the domain, the migrations
web/        React app and Astro site (not started)
deploy/     Dockerfile, compose file, Helm chart
```

## Running the API

Postgres 15 or later, and a Rust toolchain matching `api/rust-toolchain.toml`.

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

The web app builds with `VITE_SOURCE_URL` pointing at the source of the running version. If you
deploy a modified RoxyCloud, point it at your fork: the AGPL requires you to offer your users the
source of the version they are actually using.

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

## Development

```bash
pnpm install && pnpm run build
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

`web/dist` is embedded in the desktop build, so build the web app before touching `app/`. On Linux
the Tauri crate needs `libwebkit2gtk-4.1-dev`, `libappindicator3-dev`, `librsvg2-dev` and
`patchelf`.

See [CONTRIBUTING.md](CONTRIBUTING.md).
