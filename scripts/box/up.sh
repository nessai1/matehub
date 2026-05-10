#!/usr/bin/env bash
# MateHub box deploy -- up phase.
#
# Pulls images and brings the compose stack up. Reads .env to decide
# whether the observability profile should be activated. Polls
# `docker compose ps` for healthcheck status, then prints access URLs
# and credentials. Re-runnable: `up.sh` after `up.sh` is fine, compose
# is idempotent.
#
# Pre-requisite: ./setup.sh must have run successfully so .env exists
# with DOMAIN, secrets, and the registry login is in place.

set -euo pipefail

BUNDLE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEPLOY_DIR="$BUNDLE_ROOT/deploy"
ENV_FILE="$DEPLOY_DIR/.env"
COMPOSE_FILE="$DEPLOY_DIR/docker-compose.yml"

ANSI_CYAN=$'\033[1;36m'
ANSI_GREEN=$'\033[1;32m'
ANSI_YELLOW=$'\033[1;33m'
ANSI_RED=$'\033[1;31m'
ANSI_RESET=$'\033[0m'

log()  { printf '%s[up]%s %s\n'   "$ANSI_CYAN"   "$ANSI_RESET" "$*"; }
ok()   { printf '%s[up]%s %s\n'   "$ANSI_GREEN"  "$ANSI_RESET" "$*"; }
warn() { printf '%s[up]%s %s\n'   "$ANSI_YELLOW" "$ANSI_RESET" "$*" >&2; }
die()  { printf '%s[up] %s%s\n'   "$ANSI_RED"    "$*" "$ANSI_RESET" >&2; exit 1; }

# ── Pre-flight ───────────────────────────────────────────────────────
[[ -f "$ENV_FILE" ]]      || die "$ENV_FILE missing -- run ./setup.sh first"
[[ -f "$COMPOSE_FILE" ]]  || die "$COMPOSE_FILE missing -- archive is incomplete"
command -v docker >/dev/null 2>&1 || die "docker missing -- run ./setup.sh first"

# Source .env so DOMAIN / MATEHUB_OBSERVABILITY etc. are available to
# this script for printing URLs. Compose reads the file independently.
set -a; # shellcheck disable=SC1090
source "$ENV_FILE"
set +a

[[ -n "${DOMAIN:-}" ]] || die "DOMAIN empty in .env -- re-run ./setup.sh"

# Compose flags. Observability profile is opt-in; the env flag is
# written by setup.sh based on the operator's choice.
COMPOSE_ARGS=(-f "$COMPOSE_FILE")
if [[ "${MATEHUB_OBSERVABILITY:-0}" == "1" ]]; then
    log "observability stack enabled (--profile observability)"
    COMPOSE_ARGS+=(--profile observability)
fi

cd "$DEPLOY_DIR"

# ── Pull + up ────────────────────────────────────────────────────────
log "pulling images from ${REGISTRY_URL:-registry}"
docker compose "${COMPOSE_ARGS[@]}" pull

log "starting services"
docker compose "${COMPOSE_ARGS[@]}" up -d

# ── Health wait ──────────────────────────────────────────────────────
#
# The earlier version checked Health only and treated services without a
# healthcheck as "OK by default". That hid the case where a service
# without a healthcheck exited or got stuck in `restarting` -- the
# loop saw an empty Health column and broke out as if everything was
# fine. We now require BOTH:
#   * State == running
#   * Health is either healthy OR empty (no healthcheck declared)
#
# An exited / restarting / paused container fails the State check and
# stays in the "unhealthy" set until it recovers or the deadline hits.
log "waiting for services to become healthy (up to 180s)"
deadline=$(( $(date +%s) + 180 ))
while [[ $(date +%s) -lt $deadline ]]; do
    unhealthy=$(docker compose "${COMPOSE_ARGS[@]}" ps --format '{{.Service}} {{.State}} {{.Health}}' \
        | awk '$2 != "running" || ($3 != "healthy" && $3 != "") { print $1 }')
    if [[ -z "$unhealthy" ]]; then
        ok "all services healthy"
        break
    fi
    sleep 3
done

if [[ -n "${unhealthy:-}" ]]; then
    warn "some services still not running cleanly after timeout:"
    docker compose "${COMPOSE_ARGS[@]}" ps
    warn "continuing anyway -- check 'docker compose logs' for details"
fi

# ── Final printout ───────────────────────────────────────────────────
echo
echo "=================================================================="
echo "  MateHub is running."
echo "=================================================================="
echo
printf '  App:      %shttps://%s%s\n' "$ANSI_GREEN" "$DOMAIN" "$ANSI_RESET"
echo
printf '  %sNote:%s Caddy provisions the TLS certificate from Let'\''s Encrypt\n' \
    "$ANSI_YELLOW" "$ANSI_RESET"
echo "        on first request. The first attempt may fail with an SSL"
echo "        error — wait ~30 seconds and retry."
echo
echo "  First-time setup: open the URL above in a browser, you'll see"
echo "  a 'create the first admin' page. The first user registered on"
echo "  a fresh install becomes the hub admin."
echo

if [[ "${MATEHUB_OBSERVABILITY:-0}" == "1" ]]; then
    echo "------------------------------------------------------------------"
    echo "  Observability stack:"
    echo
    printf '  Grafana:  https://%s/grafana/\n' "$DOMAIN"
    printf '  Kibana:   https://%s/kibana/\n'  "$DOMAIN"
    echo
    echo "  Both are protected by basic auth at the Caddy edge."
    printf '    user:     %s\n' "${OBSERVABILITY_USER:-admin}"
    echo "    password: see your initial setup.sh output (NOT stored in .env)"
    echo
    echo "  Inside Grafana (after the basicauth gate) the admin user is:"
    printf '    user:     %s\n' "${GRAFANA_ADMIN_USER:-admin}"
    printf '    password: %s%s%s\n' "$ANSI_YELLOW" "${GRAFANA_ADMIN_PASSWORD:-(see deploy/.env)}" "$ANSI_RESET"
    echo
    echo "  Lost the basicauth password? Re-run ./setup.sh — it generates"
    echo "  a fresh pair and prints them with a write-this-down banner."
    echo
fi

echo "------------------------------------------------------------------"
echo "  Ops cheat-sheet:"
echo "    Tail logs:   docker compose ${COMPOSE_ARGS[*]} logs -f hub chat video"
echo "    Stop stack:  ./down.sh"
echo "    Reset all:   ./down.sh --purge   (destroys volumes -- careful)"
echo "=================================================================="
