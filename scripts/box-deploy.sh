#!/usr/bin/env bash
# MateHub box deploy -- one-shot bootstrap for a fresh VPS.
#
# 1. Make sure deploy/.env exists, copying from .env.box.example otherwise.
# 2. Validate the manual fields (DOMAIN, PUBLIC_IP, ACME_EMAIL).
# 3. Generate any blank secrets in-place using openssl.
# 4. Pull images from the configured registry and bring the stack up.
# 5. Tail healthcheck status until everything is up (or a 120s timeout fires).
#
# Pre-requisite: the host must be authenticated against REGISTRY_URL. For
# YC CR that means a one-off `docker login --username json_key
# --password-stdin cr.yandex < puller.json`. Public registries skip this.
#
# Idempotent: re-running never overwrites already-set secrets, so the same
# JWT_SECRET / passwords survive across deploys.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEPLOY_DIR="$REPO_ROOT/deploy"
ENV_FILE="$DEPLOY_DIR/.env"
ENV_TEMPLATE="$DEPLOY_DIR/.env.box.example"
COMPOSE_FILE="$DEPLOY_DIR/docker-compose.box.yml"

# ── Helpers ──────────────────────────────────────────────────────────

log() { printf '\033[1;36m[box-deploy]\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m[box-deploy] %s\033[0m\n' "$*" >&2; exit 1; }

# Generate a base64-ish secret with no shell-hostile characters.
gen_secret() { openssl rand -base64 32 | tr -d '/+=' | cut -c1-32; }

# Set KEY=value in .env, but only if the existing value is empty.
# Preserves comments and ordering.
set_if_blank() {
    local key="$1" value="$2"
    if grep -qE "^${key}=$" "$ENV_FILE"; then
        # Use a delimiter unlikely to appear in our base64-ish value.
        sed -i "s|^${key}=$|${key}=${value}|" "$ENV_FILE"
        log "generated $key"
    fi
}

# ── 1. Bootstrap .env ────────────────────────────────────────────────

if [[ ! -f "$ENV_FILE" ]]; then
    cp "$ENV_TEMPLATE" "$ENV_FILE"
    log "created $ENV_FILE from template"
    log "fill in DOMAIN, PUBLIC_IP, ACME_EMAIL and re-run this script"
    exit 0
fi

# ── 2. Validate manual fields ────────────────────────────────────────

# shellcheck disable=SC1090
set -a; source "$ENV_FILE"; set +a

[[ -n "${DOMAIN:-}"      ]] || die "DOMAIN is empty in $ENV_FILE"
[[ -n "${PUBLIC_IP:-}"   ]] || die "PUBLIC_IP is empty in $ENV_FILE"
[[ -n "${ACME_EMAIL:-}"  ]] || die "ACME_EMAIL is empty in $ENV_FILE"

# ── 3. Generate blank secrets ────────────────────────────────────────

set_if_blank POSTGRES_PASSWORD "$(gen_secret)"
set_if_blank JWT_SECRET        "$(gen_secret)"
set_if_blank S3_SECRET_KEY     "$(gen_secret)"
set_if_blank TURN_PASSWORD     "$(gen_secret)"

# ── 4. Pull & up ─────────────────────────────────────────────────────

cd "$DEPLOY_DIR"

log "pulling images from $REGISTRY_URL"
docker compose -f docker-compose.box.yml pull

log "starting services"
docker compose -f docker-compose.box.yml up -d

# ── 5. Wait for healthchecks ─────────────────────────────────────────

log "waiting for services to become healthy (up to 120s)"
deadline=$(( $(date +%s) + 120 ))
while [[ $(date +%s) -lt $deadline ]]; do
    unhealthy=$(docker compose -f docker-compose.box.yml ps \
        --format '{{.Service}} {{.Health}}' \
        | awk '$2 != "healthy" && $2 != "" { print $1 }')
    if [[ -z "$unhealthy" ]]; then
        log "all services healthy"
        log ""
        log "  open https://$DOMAIN"
        log ""
        exit 0
    fi
    sleep 3
done

log "timed out -- some services still unhealthy:"
docker compose -f docker-compose.box.yml ps
exit 1
