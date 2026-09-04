# Changelog

All notable changes to `roxycloud` will be documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [0.15.0] - 2026-09-04

### Features

- feat(auth): app passwords for WebDAV clients (#76)

## [0.14.1] - 2026-09-04

### Bug Fixes

- fix(api): user bytes never render on the app's origin (#75)

## [0.14.0] - 2026-09-04

### Features

- feat(api): serve the web app from the image (#73)

## [0.13.2] - 2026-09-04

### Bug Fixes

- fix(ci): label the image with the commit it was built from (#71)

## [0.13.1] - 2026-09-04

### Bug Fixes

- fix(deploy): put the chart version in step with the release (#69)

## [0.13.0] - 2026-09-04

### Features

- feat(client): remove a remote directory the user removed locally (#66)

## [0.12.0] - 2026-09-04

### Features

- feat(web): rename and move from the file browser (#65)

## [0.11.0] - 2026-09-04

### Features

- feat(api): collect orphaned blobs after a grace period (#64)

## [0.10.0] - 2026-09-04

### Features

- feat(api): restore and purge from the trash (#63)

## [0.9.5] - 2026-09-04

### Bug Fixes

- fix(api): cascade a trashed directory over its subtree (#61)

## [0.9.4] - 2026-09-04

### Bug Fixes

- fix(deps): update rust crate sqlx to 0.9 (#50)

## [0.9.3] - 2026-09-04

### Bug Fixes

- fix(deps): update rust crate jsonwebtoken to v11 (#7)

## [0.9.2] - 2026-09-04

### Bug Fixes

- fix(deps): update rust crate tower-http to 0.7 (#51)

## [0.9.1] - 2026-09-04

### Bug Fixes

- fix(api): answer 409 when a concurrent write takes the name (#60)

## [0.9.0] - 2026-09-04

### Features

- feat(api): rename and move nodes (#57)

## [0.8.0] - 2026-09-03

### Features

- feat(web): hide what a reader cannot do (#53)

## [0.7.0] - 2026-09-03

### Features

- feat(auth): give an account a role, and refuse a reader that writes (#52)

## [0.6.0] - 2026-09-03

### Features

- feat(web): open a file instead of downloading it (#46)

## [0.5.0] - 2026-09-03

### Features

- feat(web): a file manager, not a list (#45)

## [0.4.1] - 2026-09-03

### Bug Fixes

- fix(api): let the root node keep its empty name (#42)

## [0.4.0] - 2026-09-03

### Features

- feat(client): watch the folder and sync as it changes (#41)

## [0.3.0] - 2026-09-03

### Features

- feat(client): reconcile a local folder against the server (#39)

## [0.2.3] - 2026-09-03

### Bug Fixes

- fix: outline the cloud mark (#37)

## [0.2.2] - 2026-09-03

### Bug Fixes

- fix: use a plain cloud as the mark (#36)

## [0.2.1] - 2026-09-03

### Bug Fixes

- fix: redraw the crab claws as pincers (#35)

## [0.2.0] - 2026-09-03

### Features

- feat(site): the marketing and documentation site (#30)
- feat: add the RoxyCloud server, client, CLI and desktop shell

### Bug Fixes

- fix(deploy): serve the real binary and own the blob volume

### Refactoring

- refactor(web): move the web app from React to Angular (#29)
