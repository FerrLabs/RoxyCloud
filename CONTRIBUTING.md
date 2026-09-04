# Contributing

## Building

Postgres 15 or later and the toolchain pinned in `rust-toolchain.toml`. Every dependency
resolves from crates.io, npm, or this repository: if something here ever needs a private registry,
that is a bug, report it.

```bash
cd api && cargo test
```

The unit tests need no Postgres and no network. The suites under `api/tests` do: each one creates a
database of its own, runs the migrations into it and drops it afterwards, so they can run in
parallel without fighting over blob refcounts. They skip themselves when `DATABASE_URL` is unset,
which is why `cargo test` works on a machine with no database:

```bash
docker run -d --name roxy-db -e POSTGRES_USER=roxy -e POSTGRES_PASSWORD=roxy   -e POSTGRES_DB=roxycloud -p 5432:5432 postgres:17-alpine
DATABASE_URL=postgres://roxy:roxy@localhost:5432/roxycloud cargo test
```

`DATABASE_URL` points at any database on the server; the tests only use it to reach the server and
create their own. The role therefore needs `CREATEDB`.

## Before opening a pull request

```bash
cd api && cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
```

Pull request titles follow [Conventional Commits](https://www.conventionalcommits.org). CI rejects
anything else. Reserve `!` for changes that remove or rename something, or that reject input which
used to be valid. New endpoints, new config keys and new flags are additive.

Titles are not decoration: FerrFlow reads them on every push to main, bumps the version, writes
`CHANGELOG.md`, tags and publishes the release. There is nothing to run by hand, and nothing to bump
in a pull request. Versioning is zerover, so the major stays at `0` and a `!` moves the minor. The
whole repository shares one version, since the server, the CLI, the desktop app and both browser
surfaces ship together.

## What gets merged

A change that adds behaviour comes with tests that fail without it. Tests that assert a constant, or
re-assert what the line above just did, are noise and will be asked for removal. If you cannot
describe the bug a test would catch, the test is not earning its place.

Code carries no comments beyond `TODO` and `FIXME` markers for real follow-up work. Names, types and
file layout are expected to carry the intent instead.

If a change makes a diagram in `ARCHITECTURE.md` wrong, update it in the same pull request. A
diagram that lies is worse than no diagram, because people trust it.

## Licensing your contribution

RoxyCloud is AGPL-3.0-only. Contributions are accepted under the same licence, certified with the
[Developer Certificate of Origin](https://developercertificate.org): sign off every commit with
`git commit -s`, which appends a `Signed-off-by` line.

There is no copyright assignment and no contributor licence agreement. You keep the copyright on
what you write. The practical consequence, stated plainly so nobody is surprised later: nobody,
FerrLabs included, can relicense this project or sell proprietary exceptions to it without the
agreement of every contributor.

If you run a modified RoxyCloud as a network service, the AGPL requires you to offer your users the
source of your modified version. The web app carries a source link for exactly that reason; leave it
in place.

## Scope

RoxyCloud is deliberately narrower than Nextcloud. There is no plugin system and no extension API,
and proposals to add one will be declined. Feature requests that widen the product are better raised
as an issue before the code, so nobody spends a weekend on something that will not land.
