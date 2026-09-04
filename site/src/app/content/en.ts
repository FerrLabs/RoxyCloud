import type { SiteContent } from './site-content';

export const en: SiteContent = {
  chrome: {
    skipToContent: 'Skip to content',
    nav: { overview: 'Overview', install: 'Install', api: 'API' },
    languageName: 'English',
    languageSwitch: 'Lire en français',
    footer: {
      tagline: 'Self-hosted file storage in Rust.',
      licence: 'AGPL-3.0',
      source: 'Source',
      parent: 'A FerrLabs project',
    },
  },

  home: {
    documentTitle: 'RoxyCloud, self-hosted file storage in Rust',
    description:
      'RoxyCloud is a self-hosted file server written in Rust: a web app, a REST API and WebDAV over a content-addressed blob store, under the AGPL.',
    eyebrow: 'Self-hosted file storage',
    heading: 'Your files, on your hardware.',
    lead: 'RoxyCloud is a file server written in Rust. It keeps what you upload in a content-addressed blob store, serves it over a REST API and a web app, and runs on a machine you control.',
    install: 'Install it',
    source: 'Read the source',
    status: {
      heading: 'Where the project actually is',
      lead: 'Early, and not usable end to end yet. Here is the split, so you can decide whether to run it now or watch it for a while.',
      shippedHeading: 'Working today',
      shipped: [
        'A content-addressed blob store on local disk, deduplicating identical uploads',
        'The node tree, with per-user quotas and refcounts on the blobs',
        'Upload, download, listing, trash and restore over REST',
        'Password accounts with Argon2id, session tokens, and login from the web app, the desktop window and the CLI',
      ],
      plannedHeading: 'Not written yet',
      planned: [
        'App passwords, and the WebDAV surface they exist for',
        'Sharing by link, and search by name',
        'OIDC login',
        'The S3 backend, and the sweeper that collects orphaned blobs',
        'The sync engine behind the desktop client',
      ],
    },
    design: {
      heading: 'How it is built',
      items: [
        {
          title: 'One blob store, addressed by content',
          body: 'A file is stored under the hash of its bytes, so the same attachment uploaded twice costs one copy. Refcounts on the node tree decide when those bytes may go.',
        },
        {
          title: 'One interface, two hosts',
          body: 'The browser app and the desktop window run the same Angular build. What differs is not the interface but what it is allowed to reach, and that lives in one file.',
        },
        {
          title: 'A binary and a database',
          body: 'Migrations run on boot, configuration is environment only, and there is no plugin system to keep secure. Upgrading is replacing the image.',
        },
        {
          title: 'The source link is not decoration',
          body: 'RoxyCloud is AGPL-3.0. Run a modified version for other people and they get to read it, which is why the web app carries a link to the source of the build it came from.',
        },
      ],
    },
  },

  install: {
    documentTitle: 'Install RoxyCloud',
    description:
      'Run RoxyCloud with Docker Compose or from source: prerequisites, the environment it reads, the first administrator, and the web app build.',
    heading: 'Install RoxyCloud',
    lead: 'Two paths. Docker Compose if you want it answering on port 3001 in a few minutes, a Rust toolchain if you intend to change it.',
    requirements: {
      heading: 'Before you start',
      items: [
        'Postgres 15 or later. Compose brings its own, so you only need to provide one on the source path.',
        'A Rust toolchain matching rust-toolchain.toml, for the source path.',
        'Node 24 and pnpm, to build the web app.',
      ],
    },
    compose: {
      heading: 'With Docker Compose',
      body: 'Clone the repository, set the secrets Compose refuses to start without, and bring it up. The image builds the API from source, so the first run takes a few minutes.',
      blocks: [
        {
          caption: 'Clone and configure',
          code: `git clone https://github.com/FerrLabs/RoxyCloud.git
cd RoxyCloud

export POSTGRES_PASSWORD='a long random string'
export JWT_SECRET='a different long random string'
export BOOTSTRAP_ADMIN_EMAIL='you@example.com'
export BOOTSTRAP_ADMIN_PASSWORD='at least twelve characters'`,
        },
        {
          caption: 'Start it, and check that it answers',
          code: `docker compose -f deploy/docker-compose.yml up -d --build
curl --fail http://localhost:3001/health`,
        },
      ],
      note: 'The two bootstrap variables only do something on an empty database, where they create the first administrator. Take them back out of the environment once that has happened.',
    },
    source: {
      heading: 'From source',
      body: 'The server reads its configuration from the environment and runs its migrations on boot, so a database and a signing secret are enough to get it up.',
      blocks: [
        {
          caption: 'Run the API against a local Postgres',
          code: `DATABASE_URL=postgres://localhost/roxycloud \\
JWT_SECRET=dev-secret \\
cargo run -p roxycloud-api`,
        },
      ],
    },
    configuration: {
      heading: 'Configuration',
      body: 'Environment only. There is no configuration file to mount, and no admin page that writes one behind your back.',
      columns: ['Variable', 'Default', 'Purpose'],
      rows: [
        ['DATABASE_URL', 'required', 'Postgres connection string'],
        ['JWT_SECRET', 'required', 'HS256 secret used to sign session tokens'],
        ['PORT', '3001', 'Listen port'],
        ['BLOB_ROOT', './data', 'Root of the local blob store'],
        [
          'WEB_ROOT',
          'set in the image',
          'Directory holding the built web app, served alongside the API',
        ],
        [
          'CORS_ALLOWED_ORIGINS',
          'empty',
          'Comma-separated origins allowed to call the API from a browser, unnecessary when WEB_ROOT serves it',
        ],
        ['DEFAULT_QUOTA_BYTES', '10 GiB', 'Quota granted to an account on its first write'],
        ['SESSION_TTL_SECONDS', '12 h', 'Session token lifetime'],
        [
          'BLOB_SWEEP_INTERVAL_SECONDS',
          '1 h',
          'How often blobs nothing points at are collected, 0 disables it',
        ],
        [
          'BLOB_GRACE_PERIOD_SECONDS',
          '24 h',
          'How long an unreferenced blob is kept before collection',
        ],
        ['BOOTSTRAP_ADMIN_EMAIL', 'unset', 'Creates the first administrator on an empty database'],
        [
          'BOOTSTRAP_ADMIN_PASSWORD',
          'unset',
          'Required alongside the email, twelve characters minimum',
        ],
      ],
    },
    firstLogin: {
      heading: 'The first login',
      body: 'The CLI lives in the same workspace and talks to the same API, which makes it the shortest way to check that the administrator exists.',
      blocks: [
        {
          caption: 'Log in from the command line',
          code: `cargo run -p roxycloud-cli -- login you@example.com --password '...'`,
        },
      ],
    },
    webApp: {
      heading: 'The web app',
      body: 'The image carries it, and the API serves it from the same origin, so there is nothing to deploy separately and no CORS to configure. Hosting it yourself is still supported: build web/dist with the address of your API compiled in, serve it from any static host, and name its origin in CORS_ALLOWED_ORIGINS. Compile in the source of the version you are actually running while you are there.',
      blocks: [
        {
          caption: 'Build the browser interface for a host of your own',
          code: `pnpm install
pnpm --filter @roxycloud/web build \\
  --define ROXYCLOUD_API_URL="'https://files.example.com'" \\
  --define ROXYCLOUD_SOURCE_URL="'https://git.example.com/roxycloud'"`,
        },
      ],
    },
  },

  api: {
    documentTitle: 'The RoxyCloud API',
    description:
      'The RoxyCloud REST API: session tokens, the folder and file endpoints, and what is not implemented yet.',
    heading: 'The API',
    lead: 'One REST surface, JSON in and out, bearer tokens. WebDAV shares the same binary and the same tree under /dav, authenticated by an app password rather than a session.',
    session: {
      heading: 'Getting a token',
      body: 'Every /v1 route except login expects an Authorization header. A session lasts twelve hours unless SESSION_TTL_SECONDS says otherwise, and there is no refresh: log in again. The account carries a role, admin, member or reader, and a reader is answered 403 on upload and delete rather than being trusted to hide the buttons.',
      blocks: [
        {
          caption: 'Exchange a password for a session',
          code: `curl -X POST http://localhost:3001/v1/auth/login \\
  -H 'Content-Type: application/json' \\
  -d '{"email":"you@example.com","password":"..."}'`,
        },
        {
          caption: 'What comes back',
          code: `{
  "token": "eyJhbGciOiJIUzI1NiIs...",
  "expires_in": 43200,
  "user": {
    "id": "b7f0c2de-6a1e-4d5f-9a0b-2f9c1d7e4a30",
    "email": "you@example.com",
    "display_name": "you",
    "role": "admin",
    "is_admin": true,
    "created_at": "2026-09-03T08:00:00Z"
  }
}`,
        },
      ],
    },
    endpoints: {
      heading: 'Endpoints',
      body: 'This is the whole surface, not a selection from it.',
      columns: ['Method', 'Path', 'What it does'],
      rows: [
        ['GET', '/health', 'Liveness, and the only route that takes no token'],
        ['POST', '/v1/auth/login', 'Exchange an email and a password for a session token'],
        ['GET', '/v1/auth/me', 'The authenticated account'],
        ['GET', '/v1/folders', 'List the root'],
        ['GET', '/v1/folders/{*path}', 'List a directory'],
        ['PUT', '/v1/files/{*path}', 'Upload, creating the parent directories'],
        ['GET', '/v1/files/{*path}', 'Download'],
        ['DELETE', '/v1/files/{*path}', 'Move to the trash'],
        ['POST', '/v1/move', 'Rename a node, or move it under another directory'],
        ['GET', '/v1/app-passwords', 'The credentials this account has minted'],
        ['POST', '/v1/app-passwords', 'Mint one, shown once'],
        ['DELETE', '/v1/app-passwords/{id}', 'Revoke one, taking effect immediately'],
        ['GET', '/v1/trash', 'What the account has deleted'],
        ['POST', '/v1/trash/{id}/restore', 'Bring it back, with the directories it needs'],
        ['DELETE', '/v1/trash/{id}', 'Delete it for good, and release its bytes'],
      ],
    },
    transfers: {
      heading: 'Moving bytes',
      body: 'An upload is the raw file in the request body, and it answers 201 with the node and an ETag. A download streams the bytes back with that same ETag, and a delete answers 204 once the node sits in the trash.',
      blocks: [
        {
          caption: 'Upload a file',
          code: `curl -X PUT http://localhost:3001/v1/files/notes/todo.md \\
  -H "Authorization: Bearer $TOKEN" \\
  --data-binary @todo.md`,
        },
        {
          caption: 'Rename a file, then move it into a directory',
          code: `curl -X POST http://localhost:3001/v1/move \\
  -H "Authorization: Bearer $TOKEN" \\
  -H "Content-Type: application/json" \\
  -d '{"from": "/draft.md", "to": "/todo.md"}'

curl -X POST http://localhost:3001/v1/move \\
  -H "Authorization: Bearer $TOKEN" \\
  -H "Content-Type: application/json" \\
  -d '{"from": "/todo.md", "to": "/notes/todo.md"}'`,
        },
        {
          caption: 'List a directory, then download from it',
          code: `curl http://localhost:3001/v1/folders/notes \\
  -H "Authorization: Bearer $TOKEN"

curl -O http://localhost:3001/v1/files/notes/todo.md \\
  -H "Authorization: Bearer $TOKEN"`,
        },
      ],
    },
    gaps: {
      heading: 'What is missing',
      body: 'Search, share links, app passwords, WebDAV and resumable uploads are tracked as issues, and none of them are implemented. A path that is not in the table above answers 404.',
    },
  },
};
