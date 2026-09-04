# RoxyCloud chart

Deploys the RoxyCloud API: one replica, one volume for the blobs, and a Secret for the two values
it will not start without.

## What it does not do

**It does not run Postgres.** Point it at a database you already have, whether that is an operator
like CloudNativePG, a managed instance, or a Postgres you installed yourself. A bundled subchart
would be a second database to upgrade and back up, and everyone running Kubernetes seriously has an
opinion about that already.

**It does not serve the web app.** The image carries the API. `web/` builds to static files that any
web server or object store can host, with the API URL compiled in.

**It does not publish an image.** Nothing pushes `ghcr.io/ferrlabs/roxycloud-api` yet, so build and
push your own until it does:

```bash
docker build -f deploy/Dockerfile -t your.registry/roxycloud-api:0.12.0 .
docker push your.registry/roxycloud-api:0.12.0
```

## Installing

```bash
helm install roxycloud deploy/helm/roxycloud \
  --set image.repository=your.registry/roxycloud-api \
  --set database.url='postgres://roxycloud:password@postgres/roxycloud' \
  --set jwt.secret="$(openssl rand -hex 32)" \
  --set bootstrapAdmin.email=you@example.com \
  --set bootstrapAdmin.password='at least twelve characters'
```

Both secrets can come from a Secret you manage instead, which is what you want if they are already
in External Secrets or a sealed secret:

```yaml
database:
  existingSecret: roxycloud-database
  existingSecretKey: url
jwt:
  existingSecret: roxycloud-jwt
  existingSecretKey: secret
```

The bootstrap administrator is created once, on a database with no accounts, and ignored after that.
Rotating `jwt.secret` invalidates every session token in circulation.

## One replica

`replicas` is not a value. The blob store is a directory, the claim is `ReadWriteOnce`, and two pods
writing the same tree would corrupt refcounts that Postgres believes are true. The deployment uses
the `Recreate` strategy for the same reason: a rolling update would try to attach the volume twice.
That constraint lifts when the S3 backend lands.

## Values

| Value | Default | Purpose |
|---|---|---|
| `image.repository` | `ghcr.io/ferrlabs/roxycloud-api` | Image to run |
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
| `persistence.storageClass` | cluster default | Class of the created claim |
| `service.type` | `ClusterIP` | Service type |
| `service.port` | `3001` | Port for the service and the container |
| `ingress.enabled` | `false` | Create an Ingress |
| `ingress.host` | none | Required when the Ingress is on |
| `ingress.className` | cluster default | Ingress class |
| `ingress.tls.enabled` | `false` | Serve the host over TLS |
| `ingress.tls.secretName` | none | Required when TLS is on |
| `resources` | none | Container requests and limits |

## Checking a change to the chart

```bash
helm lint deploy/helm/roxycloud --set database.url=x --set jwt.secret=y
helm template roxycloud deploy/helm/roxycloud --set database.url=x --set jwt.secret=y | kubeconform -strict -summary
```
