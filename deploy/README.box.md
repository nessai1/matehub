# MateHub — boxed deployment

Self-contained MateHub on a single VPS. No SaaS control plane, no general
service, no Kubernetes. Everything (Postgres, ScyllaDB, Redis, NATS, MinIO,
TURN, the Vite SPA, four Rust services, Caddy) runs in one
`docker compose` stack behind one domain.

For production multi-tenant deploys see `deploy/helm/`.

---

## Requirements

| | |
|---|---|
| **VPS** | 4 GB RAM, 2 vCPU, ~30 GB SSD |
| **OS** | any with Docker 24+ and the Compose plugin |
| **DNS** | one A-record pointing at the VPS public IP |
| **Open ports** | `80/tcp`, `443/tcp`, `4001/udp`, `3478/udp+tcp`, `49152-65535/udp` |

The 4 GB target is *tight* — ScyllaDB alone wants 768 MB heap and the
Rust build chain peaks above 2 GB. Don't run the build with services
already up; `box-deploy.sh` schedules it so it doesn't matter.

---

## First run

```sh
# 1. DNS — point dima.matehub.io at the VPS A-record beforehand.

git clone <repo> matehub && cd matehub

# 2. Generate the env file template
./scripts/box-deploy.sh
# (creates deploy/.env, asks you to fill in the manual fields, exits)

# 3. Edit deploy/.env — set DOMAIN, PUBLIC_IP, ACME_EMAIL.

# 4. Build & start.
./scripts/box-deploy.sh
```

The second invocation:
- generates the four blank secrets (`POSTGRES_PASSWORD`, `JWT_SECRET`,
  `S3_SECRET_KEY`, `TURN_PASSWORD`) using `openssl rand`,
- runs `docker compose build` (one cargo build covers all four binaries —
  ~10 min cold, sub-second incremental thanks to BuildKit layer cache),
- brings the stack up,
- polls `docker compose ps` until everything reports `healthy`.

When it's done you can hit `https://dima.matehub.io`. Caddy fetches a
Let's Encrypt cert on first request, no extra step.

---

## Anatomy of a request

```
   browser ──► https://dima.matehub.io  ──►  Caddy :443 (TLS, ACME)
                                               │
              ┌────────────────────────────────┼───────────────────────────┐
              ▼                                ▼                           ▼
        /api/hub/* → hub:3002          /api/chat/* → chat:3003       /api/video/* → video:4000
        (REST + WS presence)            (REST + WS gateway)           (REST + WS signalling)
              │                                                            │
              ▼                                                            ▼
        postgres, redis,                                             nats — voice.occupancy
        nats, minio                                                  (str0m media goes UDP
                                                                      direct to host:4001)

   browser ──► turn:dima.matehub.io:3478 ──► coturn (network_mode: host)
   (only when client's NAT blocks direct UDP to host:4001)
```

Caddy strips the `/api/<svc>` prefix before forwarding, so each upstream
sees its native paths (`/v1/...`, `/ws/...`).

---

## Why TURN + a domain are mandatory (not optional)

- **HTTPS** — `getUserMedia` and `MediaDevices` are gated behind a secure
  context. On `http://<ip>` the browser silently refuses to grant
  camera/mic, so calls just won't start. Subdomain + Let's Encrypt fixes
  this for free.
- **TURN** — clients on symmetric NAT (mobile networks, hotel Wi-Fi,
  some corporate firewalls) can't establish a direct UDP path to
  `host:4001` even with a public IP. Without a TURN relay those users
  see "connecting…" forever. Coturn runs with `lt-cred-mech` and a
  randomly-generated password baked into the SPA bundle so it's not an
  open relay for the wider internet.

---

## Common operations

```sh
# tail logs
docker compose -f deploy/docker-compose.box.yml logs -f hub chat video

# restart one service after a code change
docker compose -f deploy/docker-compose.box.yml up -d --build hub

# wipe everything (data volumes too — careful)
docker compose -f deploy/docker-compose.box.yml down -v
```

To rebuild the SPA after a frontend change:
```sh
docker compose -f deploy/docker-compose.box.yml up -d --build --force-recreate frontend-build
```
The `frontend-build` container exits 0 after copying the new bundle into
the shared volume; Caddy serves it on the next request.

---

## Troubleshooting

**Browser shows "your connection is not private"** — Let's Encrypt rate
limit or DNS misconfig. Check `docker compose logs caddy`. The
`A`-record must already be live before first compose up; otherwise
ACME's HTTP-01 challenge fails and Caddy backs off for an hour.

**Calls show participants connected but no audio/video** — TURN issue.
Run `docker compose logs coturn` and check the firewall lets
`49152-65535/udp` through. On the client side, in the call debug panel,
look for `relay` ICE candidates — if there are none, TURN isn't being
reached.

**Hub returns 500 on login** — `JWT_SECRET` empty or mismatched between
hub/chat/video. They share one secret; restart the whole stack after
any `.env` edit.

**Out-of-memory during `cargo build`** — bring services down first:
`docker compose -f deploy/docker-compose.box.yml down`, then build.
A 4 GB VPS doesn't have headroom for both at once.

**ScyllaDB OOM-kills itself on tiny VPS** — drop `--memory 768M` to
`512M` and `--smp 1` stays. Below 512 MB it can't bootstrap.
