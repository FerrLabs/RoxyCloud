# Stashden chart

Deploys the Stashden API: one replica, one volume for the blobs, and a Secret for the two values
it will not start without.

## What it does not do

**It does not run Postgres.** Point it at a database you already have, whether that is an operator
like CloudNativePG, a managed instance, or a Postgres you installed yourself. A bundled subchart
would be a second database to upgrade and back up, and everyone running Kubernetes seriously has an
opinion about that already.

It serves the web app, because the image carries it. That is one deployment rather than two, and it
is why the chart has no second service, no second ingress and nothing in `config.corsAllowedOrigins`
by default. Point `ingress.host` at it and the app and the API are both there.

## The image

`ghcr.io/ferrlabs/stashden-api` is published by the release workflow, for `linux/amd64` and
`linux/arm64`, tagged with the exact version, the minor line and `latest`. The same image is still
pushed to `ghcr.io/ferrlabs/roxycloud-api`, the name it had before the project became Stashden, for
anyone who has not switched yet. The chart pins the exact
version through `appVersion`, so an upgrade moves the image and the chart together.

Building your own is still one command, which is what you want for a fork or an unreleased commit:

```bash
docker build -f deploy/Dockerfile -t your.registry/stashden-api:0.13.0 .
docker push your.registry/stashden-api:0.13.0
```

## Installing

Each release publishes this chart to `oci://ghcr.io/ferrlabs/charts/stashden` under the release
version, signed with cosign by the release workflow. Add `--version` to pin one instead of taking the
latest:

```bash
helm install stashden oci://ghcr.io/ferrlabs/charts/stashden \
  --set database.url='postgres://stashden:password@postgres/stashden' \
  --set jwt.secret="$(openssl rand -hex 32)"
```

From a checkout, with an image you built yourself:

```bash
helm install stashden deploy/helm/stashden \
  --set image.repository=your.registry/stashden-api \
  --set database.url='postgres://stashden:password@postgres/stashden' \
  --set jwt.secret="$(openssl rand -hex 32)" \
  --set bootstrapAdmin.email=you@example.com \
  --set bootstrapAdmin.password='at least twelve characters'
```

Both secrets can come from a Secret you manage instead, which is what you want if they are already
in External Secrets or a sealed secret:

```yaml
database:
  existingSecret: stashden-database
  existingSecretKey: url
jwt:
  existingSecret: stashden-jwt
  existingSecretKey: secret
```

The bootstrap administrator is created once, on a database with no accounts, and ignored after that.
Rotating `jwt.secret` invalidates every session token in circulation.

## Upgrading from the roxycloud chart

The chart was called `roxycloud` before the project became Stashden, and the chart name is part of
every resource name and of the Deployment's selector, which Kubernetes does not let an upgrade
change. A release installed from the old chart keeps its names by setting `nameOverride`:

```bash
helm upgrade roxycloud oci://ghcr.io/ferrlabs/charts/stashden \
  --reuse-values --set nameOverride=roxycloud
```

Without it, the upgrade fails on the selector, and a fresh install beside it would create an empty
claim instead of using the one that holds the blobs. The image moves to `stashden-api` on its own,
since the chart's default now points there and it is the same image.

## One replica

`replicas` is not a value. The blob store is a directory, the claim is `ReadWriteOnce`, and two pods
writing the same tree would corrupt refcounts that Postgres believes are true. The deployment uses
the `Recreate` strategy for the same reason: a rolling update would try to attach the volume twice.
That constraint lifts when the S3 backend lands.

## Uninstalling

`persistence.retain` is on, so `helm uninstall` leaves the claim and the blobs behind. Reinstalling
over a kept claim means telling the chart to adopt it rather than create a second one:

```yaml
persistence:
  existingClaim: stashden
```

Turn `persistence.retain` off if you would rather uninstall took the data with it. Nothing else in
this chart is capable of deleting the blob store.

## Values

| Value | Default | Purpose |
|---|---|---|
| `image.repository` | `ghcr.io/ferrlabs/stashden-api` | Image to run |
| `image.tag` | chart `appVersion` | Tag to run |
| `database.url` | none | Postgres connection string, required unless `database.existingSecret` is set |
| `database.existingSecret` | none | Secret already holding the connection string |
| `database.existingSecretKey` | `database-url` | Key inside that Secret |
| `jwt.secret` | none | HS256 secret for session tokens, required unless `jwt.existingSecret` is set |
| `jwt.existingSecret` | none | Secret already holding it |
| `jwt.existingSecretKey` | `jwt-secret` | Key inside that Secret |
| `bootstrapAdmin.email` | none | Creates the first administrator on an empty database |
| `bootstrapAdmin.password` | none | Minimum twelve characters |
| `config.corsAllowedOrigins` | `[]` | Origins allowed to call the API from a browser |
| `config.defaultQuotaBytes` | server default | Quota granted on first write |
| `config.sessionTtlSeconds` | server default | Session token lifetime |
| `config.blobSweepIntervalSeconds` | server default | How often orphaned blobs are collected, `0` disables it |
| `config.blobGracePeriodSeconds` | server default | How long an unreferenced blob is kept |
| `persistence.enabled` | `true` | Off means an `emptyDir`, which loses every byte when the pod moves |
| `persistence.existingClaim` | none | Claim to use instead of creating one |
| `persistence.size` | `20Gi` | Size of the created claim |
| `persistence.accessMode` | `ReadWriteOnce` | Access mode of the created claim |
| `persistence.retain` | `true` | Keep the claim when the release is uninstalled |
| `persistence.storageClass` | cluster default | Class of the created claim |
| `service.type` | `ClusterIP` | Service type |
| `service.port` | `3001` | Port for the service and the container |
| `ingress.enabled` | `false` | Create an Ingress |
| `ingress.host` | none | Required when the Ingress is on |
| `ingress.className` | cluster default | Ingress class |
| `ingress.tls.enabled` | `false` | Serve the host over TLS |
| `ingress.tls.secretName` | none | Required when TLS is on |
| `resources` | none | Container requests and limits |
| `nameOverride` | chart name | Name used for the resources and the `app.kubernetes.io/name` label |
| `fullnameOverride` | none | Full resource name, overriding the release and chart names |

## Checking a change to the chart

```bash
helm lint deploy/helm/stashden --set database.url=x --set jwt.secret=y
helm template stashden deploy/helm/stashden --set database.url=x --set jwt.secret=y | kubeconform -strict -summary
```
