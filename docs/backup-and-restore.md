# Backing up and restoring

An instance is two stores that have to agree with each other: Postgres, which holds the accounts,
the tree, the versions, the shares and the settings, and the blob store, which holds the bytes
(`BLOB_ROOT` on disk, or the S3 bucket). A backup of one without the other is not a backup.

What else matters:

- `JWT_SECRET`. Losing it signs everyone out, it does not lose data. Keep it with the secrets of the
  deployment, not in the backup of the data.
- The environment the server runs with, which is where the OIDC and storage settings come from.
- `UPLOAD_ROOT` is scratch space for resumable uploads in flight. It is not worth backing up, and a
  restored instance starts without any.

## The order matters

Take the database dump first, then copy the blobs.

A file is placed by writing its bytes to the store and then committing the row that points at them.
Dump the database first and every row in it already has its bytes when the copy starts; anything
uploaded in between is a blob nothing points at, which the sweep collects later. Copy the blobs
first and the dump can point at a blob written after the copy, which the restored store does not
have.

The other way a copy goes wrong is the sweep. It deletes a blob that nothing points at once it has
been unreferenced for `BLOB_GRACE_PERIOD_SECONDS` (24 hours by default). A blob a dump still points
at can only be deleted once the file has been removed after the dump, and then left for the whole
grace period. So a backup whose blob copy finishes within the grace period of its dump is safe. If
the copy can take longer, or the grace period is short, set `BLOB_SWEEP_INTERVAL_SECONDS=0` and
restart the server for the length of the backup.

## Docker Compose

```bash
docker compose -f deploy/docker-compose.yml exec -T db \
  pg_dump -U roxycloud -Fc roxycloud > stashden.dump

docker compose -f deploy/docker-compose.yml exec -T api \
  tar czf - -C /var/lib/roxycloud . > blobs.tgz
```

(The database and the user are still named `roxycloud`, which is what an instance created before
the rename uses. They are kept so that an upgrade does not need a migration.)

With an S3 backend, copy the bucket after the dump instead of the tar, with whatever the provider
offers: `aws s3 sync`, `rclone`, or replication to a second bucket. Turning bucket versioning on
helps against an accidental overwrite, and costs space.

With the Helm chart, dump the database the same way and snapshot the blob volume (or copy the
bucket) after it.

## Restoring

Restore the database and the blobs from the same backup, with the server stopped:

```bash
docker compose -f deploy/docker-compose.yml stop api

docker compose -f deploy/docker-compose.yml exec -T db \
  pg_restore -U roxycloud -d roxycloud --clean --if-exists < stashden.dump

docker compose -f deploy/docker-compose.yml run --rm -T --entrypoint tar api \
  xzf - -C /var/lib/roxycloud < blobs.tgz
```

Then start the server with `BLOB_SWEEP_INTERVAL_SECONDS=0`, run the check below, and put the sweep
back once it is clean. With the sweep on, a server started on a restored database collects, after
the grace period, every blob that database does not point at, which includes anything newer in a
store you have not finished sorting out.

## Checking an instance

```bash
stashden-api check                    # every referenced blob is in the store (one request each on S3)
stashden-api check --verify-content   # and its bytes still hash to its name
stashden-api check --repair           # also correct counts and usage
```

Run it where the server's environment is set, so with the compose file
`docker compose exec api stashden-api check`. It reads the same `DATABASE_URL` and blob settings the
server does.

It reports four things:

- **A referenced blob that is not in the store.** Restore it from a backup. A blob nothing points at
  is not reported, since the sweep is meant to remove those.
- **A blob whose bytes no longer match its hash**, with `--verify-content`. Reads are not re-hashed,
  so this is the only way to find a bit that rotted. It reads every referenced blob, which on a large
  store is a long job and, on S3, a real download bill.
- **A reference count that disagrees with the rows that hold the blob.** The count is what stops the
  sweep deleting a blob something still uses, so a count that is too low is a data loss waiting to
  happen, and a count that is too high is space that is never given back.
- **A quota whose usage disagrees with the files and versions the account holds.**

`--repair` rewrites the last two from the rows, which are the truth. It cannot make a missing or
rotted blob come back, and it says so. The exit status is 0 when everything is sound and 1 when
something needs a person, or when counts are off and `--repair` was not given.

Run a repair when nobody is writing: it rewrites a count from a snapshot of the rows, and an upload
committing at the same moment can be miscounted. Run the check on its own first; on a busy instance a
report of a few counts can be an upload in flight, and a second run settles it.
