# MateHub — boxed deployment

Self-contained MateHub on a single Linux server. Every component
(Postgres, ScyllaDB, Redis, NATS, MinIO, TURN, the SPA, four Rust
services, Caddy) runs as one `docker compose` stack behind one domain.
No SaaS control plane, no Kubernetes, no external dependencies beyond
the host OS.

This archive is everything you need. Three scripts:

| | |
|---|---|
| `setup.sh` | Installs Docker, generates `.env`, asks for the hostname / S3 mode / observability choice. Runs **once** per host. |
| `up.sh`    | Pulls images, brings the stack up, prints URLs and credentials. |
| `down.sh`  | Stops the stack. `--purge` also wipes data volumes (factory reset). |

---

## Minimum requirements

| Resource | Without observability | With observability (Grafana + ELK) |
|---|---|---|
| **OS** | Linux: Debian 12+ / Ubuntu 22.04+ / RHEL 9 / AlmaLinux 9 / Rocky 9 / Fedora | same |
| **CPU** | 2 vCPU | 4 vCPU |
| **RAM** | 4 GB | 8 GB |
| **Disk** | 30 GB SSD | 50 GB SSD |
| **Network** | 1 public IPv4, ports listed below | same |
| **Tools** | bash 4+, openssl, curl (setup.sh installs anything else missing) | same |

Why the bigger numbers with observability: Elasticsearch alone wants 1 GB
of heap (and similar off-heap), Grafana/Kibana add another ~500 MB
combined, and we have to leave headroom for the ingest path
(filebeat → ES). On a 4 GB box with the metrics stack on, you'll get
OOMs under any real load.

### Open these ports on the host firewall **and** any cloud security group

| Port | Proto | Purpose |
|---|---|---|
| 80 | TCP | ACME HTTP-01 challenge + redirect to 443 |
| 443 | TCP | HTTPS |
| 4001 | UDP | SFU (video) media — direct UDP, **cannot** be proxied through Caddy |
| 3478 | UDP + TCP | TURN |
| 49152-65535 | UDP | TURN media relays for clients on symmetric NAT |

### DNS

One A-record pointing the chosen domain at the host's public IPv4.
**This must resolve before you run `up.sh`** — Caddy uses Let's Encrypt's
HTTP-01 challenge on first start, and a wrong/missing DNS answer triggers
LE rate-limits that lock you out of certificate issuance for an hour.

Verify with `dig +short YOUR.DOMAIN` — should return your public IP.

---

## Install

```sh
tar -xzf matehub-box-*.tar.gz
cd matehub-box-*

./setup.sh
# Prompts:
#   - S3: local (bundled MinIO) or external (AWS / Yandex / your own bucket)
#   - Observability: yes/no (Grafana + Kibana + Prometheus + Elasticsearch)
#   - DOMAIN, PUBLIC_IP, ACME_EMAIL
# After: red banner reminds you to verify DNS resolves.

./up.sh
# Pulls images from cr.yandex (using the bundled puller.json),
# brings the stack up, polls healthchecks (~2 minutes for first run),
# prints the URL and credentials.
```

Open `https://<your-domain>` in a browser. The first user to register
becomes the hub admin — no separate setup step.

---

## Re-running setup or up

Both scripts are idempotent.

* `./setup.sh` after a successful run only fills **blank** values in
  `.env`. Existing secrets, the hostname, the observability choice all
  survive. Useful when you want to enable observability later: re-run,
  answer "yes" this time.
* `./up.sh` after the stack is already running is a no-op (Compose
  detects no-changes). Use it after any `.env` edit to apply.

---

## External S3

Pick "external" at the prompt. `setup.sh` writes blank placeholders for
every S3 field; you edit `deploy/.env` to fill them in:

```
S3_ENDPOINT=https://storage.yandexcloud.net
S3_REGION=ru-central1
S3_PUBLIC_URL=https://storage.yandexcloud.net      # if bucket is public
S3_ACCESS_KEY_ID=...
S3_SECRET_ACCESS_KEY=...
S3_BUCKET_HUB_ASSETS=matehub-hub-assets
S3_BUCKET_CHAT_MEDIA=matehub-chat-media
```

If the bucket isn't publicly readable, set `S3_PUBLIC_URL` to a CloudFront
/ Yandex Cloud CDN endpoint that fronts it. The bundled MinIO doesn't
start in external mode.

---

## Observability access

When enabled, both Grafana and Kibana are reachable through Caddy with
basic-auth. The credentials are printed by `up.sh` and stored in
`deploy/.env`:

* `https://<domain>/grafana/` — dashboards (Prometheus + Elasticsearch
  pre-wired as datasources). Two layers of auth: Caddy basicauth →
  Grafana admin login.
* `https://<domain>/kibana/` — log search across all matehub services.
  Filebeat tails `LOG_FORMAT=json` stdout from each container and
  ships to ES; you'll see one index per service per day
  (`matehub-hub-2026.05.10`, `matehub-chat-2026.05.10`, ...).

To enable the in-call video debug panel on a deployed box: open the
browser DevTools console and run `enableVideoTrace()`. The page
reloads with the diagnostic panel visible. Run `disableVideoTrace()`
to turn it off again.

---

## Common operations

```sh
# Tail logs
docker compose -f deploy/docker-compose.yml logs -f hub chat video

# Restart a single service after editing .env
docker compose -f deploy/docker-compose.yml up -d --force-recreate hub

# Update to a newer image (after bumping HUB_VERSION etc. in .env)
./up.sh

# Stop everything, keep data
./down.sh

# Wipe the entire stack
./down.sh --purge
```

---

## Troubleshooting

**Browser shows "your connection is not private"**
Let's Encrypt rate-limit, or DNS still didn't propagate before `up.sh`.
`docker compose logs caddy` will say so explicitly. Wait an hour,
verify `dig +short YOUR.DOMAIN`, then `docker compose restart caddy`.

**Calls show "connecting…" indefinitely**
Likely a TURN issue. Check `docker compose logs coturn` and confirm
the firewall allows `49152-65535/udp`. In the in-call debug panel
(see `enableVideoTrace()` above) look for `relay` ICE candidates —
their absence means TURN can't be reached.

**`hub` returns 500 on login**
`JWT_SECRET` mismatch between hub/chat/video, or empty. They share
one secret; restart the whole stack after editing `.env`.

**MinIO upload returns 200 but the file appears as broken in the chat**
The browser can't fetch `http://minio:9000/...`. This release fixed
that — backend now returns `https://<domain>/s3/<bucket>/<key>` and
Caddy proxies. If you still see the bug: confirm `S3_PUBLIC_URL` in
`deploy/.env` resolves to the public path and that Caddy's `/s3/*`
route is intact (`docker compose logs caddy`).

**Out of memory during first start with observability**
Elasticsearch is the hungriest service. If you're running on a 4 GB
host you can either drop the observability profile (re-run setup,
answer "no") or increase the host RAM.

---

## Removing the box

```sh
./down.sh --purge       # wipes data volumes
docker logout cr.yandex # forgets the registry credential
```

The puller key in `puller.json` only has the `images.puller` IAM role —
worst case if it leaks, somebody pulls a copy of the same images you
already have. No write/admin/billing access.
