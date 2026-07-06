# Box release: signed-archive deployment + observability + MinIO/videoTrace fixes

## Summary

This PR turns "MateHub on a single Linux server" from a contributor-only
flow into a deliverable artifact. After it lands, tagging `box/vX.Y.Z`
produces three Linux-flavoured tarballs as GitHub Release assets, each
containing everything a customer needs to bring up MateHub on a fresh
host: docker installer, registry credential, compose stack, and three
operator scripts. The customer types `tar -xzf …`, `./setup.sh`,
`./up.sh` and they're done.

While building that pipeline I also fixed two latent bugs the box flow
exposed (MinIO returning unreachable URLs to the browser; the in-call
video-debug panel being unreachable on a deployed instance) and folded
in an opt-in observability stack (Grafana + Kibana + Prometheus +
Elasticsearch + Filebeat).

The PR is intentionally one chunk because the pieces only make sense
together — the archive is useless without the bug fixes, and the
operator scripts only earn their complexity once the observability
profile exists. Splitting would force a sequence of "broken middle"
states across master.

---

## What to focus on when reviewing

The diff is large but most of it is well-isolated. The parts worth
careful eyes:

1. **`packages/common-rust/src/storage.rs`** — `S3_PUBLIC_URL` field on
   `S3Storage` plus `from_env()` reading it with fallback to `endpoint`.
   This is the only Rust behavioural change in the PR. Previously
   `public_url()` returned `http://minio:9000/<bucket>/<key>` on box
   deploys (compose-internal hostname, browsers can't reach it); now
   it returns `https://<domain>/s3/<bucket>/<key>` reverse-proxied by
   Caddy. Three unit tests pin the override / fallback / round-trip
   semantics.

2. **`scripts/pack-box-archive.sh`** — the build-time machinery. Resolves
   the latest `<service>/X.Y.Z` git tag for hub/chat/video/transcoder,
   bakes those versions into `.env.box.example` inside the staging
   directory, and tars it. Worth eyeballing: the `sed -i.bak` substitutions
   for the version pinning, the file layout (no symlinks — `tar` portability
   bites here), and the `chmod 0600 puller.json` step.

3. **`scripts/box/setup.sh`** — the operator-facing entry point. Three
   idempotency invariants matter:
   - Re-running after success must not regenerate secrets (`set_env_blank`
     only fills empty values; existing ones stay).
   - Re-running after success must not redo `docker login` (skip if
     `~/.docker/config.json` already contains a `cr.yandex` credential).
   - The `docker` group add path requires a fresh login session, so the
     script exits with a clear message instead of failing later.

4. **`.github/workflows/release.yml`** — added matrix job
   `build-box-archives` triggered on `box/v*` tags. The `if:` guards on
   both jobs (`!startsWith` / `startsWith`) ensure per-service tags don't
   accidentally run the archive job and vice versa. New repo secret
   required: `YC_PULLER_KEY` (raw JSON SA key for an SA holding only
   `container-registry.images.puller`).

5. **`deploy/docker-compose.box.yml`** — observability profile addition.
   Five services under `profiles: ["observability"]` (so plain `up` is
   unchanged), no host-port maps for any of them (everything goes
   through Caddy), and `matehub.service` labels on hub/chat/video/transcoder
   so Filebeat's docker-autodiscover picks them up.

6. **`frontend/lib/video-trace.ts` + `main.tsx`** — exposes
   `window.enableVideoTrace()` / `disableVideoTrace()` globally. The
   side-effect import in `main.tsx` is the only reason the function
   is reachable from the browser console on every page (not just inside
   a video call). Worth a glance to confirm this is the discoverability
   pattern you want.

---

## Architecture

### MinIO public-URL fix

The bug: on box deploys, `S3Storage::public_url()` was building URLs
from `S3_ENDPOINT` (= `http://minio:9000`, compose-internal). The
backend handed those URLs to the browser; the browser can't resolve
`minio` and rendered every uploaded asset as broken.

The fix is one new env var (`S3_PUBLIC_URL`) propagated end-to-end:

```rust
pub struct S3Storage {
    pub client: Client,
    pub bucket: String,
    pub endpoint: String,           // compose-internal, used by SDK
    pub public_url_base: String,    // browser-facing, used by public_url()
}

pub async fn from_env(bucket_env: &str) -> Self {
    let endpoint = std::env::var("S3_ENDPOINT")...;
    let public_url_base = std::env::var("S3_PUBLIC_URL").unwrap_or_else(|_| endpoint.clone());
    ...
}

pub fn public_url(&self, key: &str) -> String {
    format!("{}/{}/{}", self.public_url_base, self.bucket, key)
}
```

For AWS / public-Yandex deployments the two values are identical and
behaviour is unchanged. For box deploys, compose substitutes
`S3_PUBLIC_URL=https://${DOMAIN}/s3` by default, and a new Caddyfile
route reverse-proxies `/s3/*` to `minio:9000`. MinIO buckets `hub-assets`
and `chat-media` are already `mc anonymous set download` (configured
in `minio-init`), so anonymous GET through the Caddy proxy works.

External-S3 operators leave `S3_PUBLIC_URL` blank in `.env`, which
makes compose fall back to the endpoint — works correctly for AWS S3 /
public Yandex Cloud Storage without further config.

### videoTrace in production

Previously: `import.meta.env.DEV && <CallDebugPanel client={client} />`
in `video-workspace.tsx`. The panel was simply absent in production
bundles.

Now: a localStorage flag (`matehub.video_trace_enabled`) acts as the
runtime gate, and three globals expose the toggle:

```js
window.enableVideoTrace()    // sets the flag, reloads, panel appears
window.disableVideoTrace()   // unsets, reloads
window.isVideoTraceEnabled()
```

Side-effect import in `src/main.tsx` registers the globals at app
startup, so support / on-call can type the function name in DevTools
on any page (login, hub list, in-call) and have it work. The dev
behaviour is unchanged — `import.meta.env.DEV` still mounts the panel
unconditionally on local development.

### Observability stack

Five services under `profiles: ["observability"]` in
`deploy/docker-compose.box.yml`, content adapted from
`deploy/observability/docker-compose.yaml` with three box-specific
changes:

1. **No host-port maps.** Grafana/Kibana go through Caddy under
   `/grafana/*` and `/kibana/*` with basicauth (env-driven hash); ES,
   Prometheus, and Filebeat stay internal to the compose network.

2. **`prometheus.box.yml`** — separate from the dev `prometheus.yml`
   because dev targets `host.docker.internal:PORT` (services run on
   the host via `cargo run`), while box targets `hub:3002` /
   `chat:3003` / `video:4000` (compose service names; everything is
   sibling containers).

3. **Filebeat config reused as-is** — autodiscover picks up containers
   labelled `matehub.service: <name>` and decodes JSON from
   `LOG_FORMAT=json` stdout. Both pieces are new on the box services
   (4 service blocks, +1 `labels:`, +1 env var each).

Caddy auth is one bcrypt-hashed credential gating both Grafana and
Kibana, generated by `setup.sh` on opt-in. The compose env has a
default-deny placeholder hash so a misconfigured box (observability
profile up but `OBSERVABILITY_PASSWORD_HASH` empty) can't be silently
auth-bypassed.

### Operator scripts

```
matehub-box-<variant>-<version>/
├── setup.sh                # one-time host prep
├── up.sh                   # pull + up + healthcheck wait + URLs/creds
├── down.sh                 # stop; --purge wipes volumes
├── README.md (= en)
├── README.en.md
├── README.ru.md
├── puller.json             # YC SA pull-only credential
└── deploy/
    ├── docker-compose.yml  # renamed from docker-compose.box.yml
    ├── Caddyfile
    ├── .env.box.example    # versions pre-filled, secrets blank
    └── observability/...
```

`setup.sh` flow:
1. `/etc/os-release` family detection (debian/ubuntu/rhel — single
   archive content for now, distros differ only in name).
2. `command -v docker` check; `curl -fsSL https://get.docker.com | sh`
   if missing.
3. User in `docker` group? If not, `usermod -aG docker $USER` and exit
   with a "log out and back in" message — Docker requires a fresh
   session for the new gid to attach.
4. `docker login cr.yandex < puller.json`, skipped if already logged in.
5. Prompts (or CLI flags `--domain`/`--public-ip`/etc. for unattended
   mode): S3 mode → observability → DOMAIN/PUBLIC_IP/ACME_EMAIL.
6. `set_env_blank` fills `POSTGRES_PASSWORD` / `JWT_SECRET` /
   `S3_SECRET_KEY` / `TURN_PASSWORD` / observability creds with
   `openssl rand -hex 32`.
7. Persists `MATEHUB_OBSERVABILITY=0|1` so `up.sh` knows whether to
   pass `--profile observability`.
8. Loud red banner: "verify DNS resolves before `up.sh` or you'll burn
   an hour on a Let's Encrypt rate-limit lockout."

Scope-creep guard: setup.sh does not bring the stack up. The
operator runs `up.sh` after they've verified DNS / firewalled
ports / edited external-S3 fields if applicable. This is deliberate —
silently auto-upping after generating credentials makes failure modes
much harder to debug.

### Release pipeline

`release.yml` learns one new tag pattern (`box/v*`) and one new job
(`build-box-archives`) that runs on it as a 3-way matrix
(debian/ubuntu/rhel). The job materialises `puller.json` from
`secrets.YC_PULLER_KEY`, runs `pack-box-archive.sh --variant <v>
--version <ver>`, and uploads via `softprops/action-gh-release@v2`.

The existing `build-push` job got `if: ${{ !startsWith(github.ref_name,
'box/') }}` so per-service tags don't accidentally hit the archive
path. A `jq -e` shape check on the materialised secret fails fast with
a clear message if `YC_PULLER_KEY` is missing or malformed.

---

## Test coverage

### Unit tests (in `packages/common-rust`)

- `public_url_uses_public_base_when_separate_from_endpoint` — box-shape
  configuration. Pins the bug fix.
- `public_url_falls_back_to_endpoint_when_base_equals_endpoint` —
  AWS-shape configuration. Confirms no regression for the common case.
- `key_from_url_strips_public_base_not_endpoint` — round-trip property.
  The previous implementation used `endpoint`, which would silently
  drop every key once the box flips to a public base — explicit test.

### Integration / scaffolding tests still green

- `cargo test --all -- --test-threads=1` — 16 binaries, ~270 tests,
  zero failures. Including the existing `invite_links_tests` (15) and
  `pool_isolation_tests` (1) that this branch doesn't touch.

### Compose validation

- `docker compose -f deploy/docker-compose.box.yml config --quiet` —
  clean (only env-not-set warnings, expected without a `.env`).
- Same with `--profile observability` — clean.

### Pack-script smoke test

- `./scripts/pack-box-archive.sh --variant ubuntu --version v0.0.1
  --puller ./puller.json` produces a 26 KB tarball with the expected
  layout (verified via `tar -tzf`). Versions baked into the archive's
  `.env.box.example` match the latest `<service>/X.Y.Z` tags.

### What I did **not** test

- **End-to-end on a real VM.** The full setup→up→browser flow needs a
  live YC tenant, a registered domain with DNS, ACME-reachable ports.
  I have neither in my dev box. The scripts are syntax-clean
  (`bash -n`), the compose validates, and the resolved imageVersions
  point at images that exist in the registry, but a smoke run on
  Ubuntu 22.04 / Rocky 9 before the first customer ships is
  recommended.
- **Filebeat → ES → Kibana ingest path.** Same reason — needs
  observability profile up on a real host. The config is copied
  verbatim from `deploy/observability/docker-compose.yaml`, which
  works in dev, so the risk is low.

---

## Decision log

- **One PR vs three** — chose one. The scope splits cleanly into
  (MinIO fix) + (videoTrace) + (box pipeline + observability), but the
  archive isn't useful until the bug fixes land, and the observability
  stack needs the same env / Caddy plumbing as the rest of the box
  flow. Three PRs would have produced two "broken middle" states on
  master with no operational benefit.

- **`S3_PUBLIC_URL` as env override vs separate config struct** —
  chose env. One ergonomic precedent (`S3_ENDPOINT`, `S3_REGION`, etc.),
  no schema migration, fallback semantics are obvious in the code.
  Compose substitutes a sane default for the bundled-MinIO case, so
  operators don't have to set it explicitly.

- **Observability as a profile vs separate compose file** — chose
  profile. Compose composition with `-f` is fragile in setup scripts
  (extra arg threading, ordering surprises); profiles give the same
  opt-in semantics inside one file.

- **Caddy basicauth vs in-app auth for Grafana/Kibana** — chose Caddy
  basicauth as the single auth gate. Grafana keeps its own admin
  login as a second layer; Kibana is x-pack-security-off (matches the
  dev compose). One credential to surface, one place to rotate.

- **Three archive variants vs one** — kept three. Per the team's
  delivery preference (separate downloadable per stated distro). Content
  is identical today; if RHEL-family ever needs a different installer
  bootstrap, the matrix already exists.

- **`box/v*` tag scheme vs reusing per-service tags** — separate. The
  archive version is independent of any single image version (it pins
  the latest of each at archive-build time). Tying it to a service
  name would be misleading.

---

## Out of scope (deliberately)

- **Manual VM smoke test** (see Test coverage). First customer ship
  should include this; not blocking the PR.
- **Bundled Grafana dashboards / Kibana saved searches.** The
  observability stack boots empty — operators import dashboards
  themselves. Adding default dashboards is a follow-up that doesn't
  block the deployment story.
- **TLS for internal traffic** (Caddy ↔ services, ES ↔ Filebeat).
  Everything inside the compose network is plaintext; the perimeter
  (Caddy ↔ browser) is TLS. Box deploys live on one host, this is
  the right trade-off.
- **Automatic image-version refresh on hot config reload.** Operators
  bump `HUB_VERSION` etc. in `.env` and run `up.sh` to pull. Not
  doing the "watch the registry, auto-rolling-update" thing — the
  failure mode (`ImagePullBackoff` at 3am) is worse than the
  ergonomic gain.

---

## Files

```
ADDED:
  scripts/box/setup.sh                                 operator entry: docker, login, .env, secrets
  scripts/box/up.sh                                    pull + up + healthcheck wait + creds printout
  scripts/box/down.sh                                  stop; --purge for full wipe
  scripts/box/README.en.md                             customer-facing instructions (English)
  scripts/box/README.ru.md                             customer-facing instructions (Russian)
  scripts/pack-box-archive.sh                          build-time tarball assembler
  deploy/observability/prometheus.box.yml              box-targets Prometheus config
  frontend/lib/video-trace.ts                          enableVideoTrace globals + localStorage flag

MODIFIED:
  packages/common-rust/src/storage.rs                  S3_PUBLIC_URL support + 3 unit tests
  deploy/docker-compose.box.yml                        observability profile, S3_PUBLIC_URL,
                                                       matehub.service labels, LOG_FORMAT=json
  deploy/Caddyfile                                     /s3/*, /grafana/*, /kibana/* routes
  deploy/.env.box.example                              S3_PUBLIC_URL + observability env block
  deploy/README.box.md                                 archive flow section, requirements table
  .github/workflows/release.yml                        box/v* trigger + build-box-archives matrix job
  frontend/components/hub/video-workspace/
    video-workspace.tsx                                gating widened to localStorage flag
  frontend/src/main.tsx                                side-effect import for window.enableVideoTrace
```

---

## How to test by hand

### Local pack smoke

```sh
./scripts/pack-box-archive.sh \
    --variant ubuntu --version v0.0.1-test \
    --puller ./puller.json --out dist
tar -tzf dist/matehub-box-ubuntu-v0.0.1-test.tar.gz | head
```

### Real-VM smoke (recommended before first customer ship)

```sh
# On a fresh Ubuntu 22.04 / Rocky 9 VPS with a domain pointed at it:
tar -xzf matehub-box-ubuntu-vX.Y.Z.tar.gz
cd matehub-box-ubuntu-*
./setup.sh   # answers: 1 (local S3), y (observability), <domain>, <ip>, <email>
# log out / back in (docker group), then:
./setup.sh   # second pass — registry login + secret gen
./up.sh      # ~2 min for pulls + healthchecks
# open https://<domain> — first registered user becomes admin
# open https://<domain>/grafana/ — basicauth then Grafana login
# upload an avatar in chat — verify URL is https://<domain>/s3/...
# in DevTools console: enableVideoTrace() — debug panel appears on next call
```

### CI release smoke (after merging this PR)

1. Add repo secret `YC_PULLER_KEY` = full JSON SA key (one-line
   contents of `puller.json`).
2. `git tag box/v0.0.1-rc1 && git push origin box/v0.0.1-rc1`.
3. Watch the `build-box-archives` matrix in Actions; verify three
   tarballs land as assets on the auto-created Release page.
