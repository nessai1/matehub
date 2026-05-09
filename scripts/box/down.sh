#!/usr/bin/env bash
# MateHub box deploy -- down phase.
#
# Stops the compose stack. By default volumes (Postgres, ScyllaDB, MinIO,
# Caddy ACME state, observability) are kept so a follow-up `up.sh` boots
# the same data. Pass `--purge` to also wipe volumes -- this is the
# "factory reset" option, useful for re-testing from scratch.

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

log()  { printf '%s[down]%s %s\n'   "$ANSI_CYAN"   "$ANSI_RESET" "$*"; }
ok()   { printf '%s[down]%s %s\n'   "$ANSI_GREEN"  "$ANSI_RESET" "$*"; }
die()  { printf '%s[down] %s%s\n'   "$ANSI_RED"    "$*" "$ANSI_RESET" >&2; exit 1; }

PURGE=0
FORCE=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --purge) PURGE=1; shift;;
        --force) FORCE=1; shift;;
        -h|--help)
            cat <<USAGE
Usage: $0 [--purge] [--force]

  (no flags)   stop services, keep volumes -- next up.sh resumes with same data.
  --purge      stop services AND remove all data volumes (Postgres, ScyllaDB,
               MinIO, observability). DESTRUCTIVE -- prompts for confirmation
               unless --force is also given.
  --force      skip the confirmation prompt for --purge.
USAGE
            exit 0;;
        *) die "unknown option: $1 (try --help)";;
    esac
done

[[ -f "$COMPOSE_FILE" ]] || die "$COMPOSE_FILE missing -- archive is incomplete"

# Source .env (if present) so we can pick up MATEHUB_OBSERVABILITY and
# include the right profile in `down`. Otherwise compose may leave
# orphan observability containers running.
COMPOSE_ARGS=(-f "$COMPOSE_FILE")
if [[ -f "$ENV_FILE" ]]; then
    set -a; # shellcheck disable=SC1090
    source "$ENV_FILE"
    set +a
    if [[ "${MATEHUB_OBSERVABILITY:-0}" == "1" ]]; then
        COMPOSE_ARGS+=(--profile observability)
    fi
fi

cd "$DEPLOY_DIR"

if [[ "$PURGE" -eq 1 ]]; then
    if [[ "$FORCE" -ne 1 ]]; then
        printf '%s[down]%s --purge will DESTROY ALL DATA in this MateHub box (Postgres, Scylla, MinIO, observability volumes). Type YES to continue: ' "$ANSI_RED" "$ANSI_RESET"
        read -r answer
        [[ "$answer" == "YES" ]] || die "aborted"
    fi
    log "stopping stack and removing volumes"
    docker compose "${COMPOSE_ARGS[@]}" down -v
    ok "stack down, all data wiped"
else
    log "stopping stack (volumes preserved)"
    docker compose "${COMPOSE_ARGS[@]}" down
    ok "stack down, data intact -- ./up.sh resumes from current state"
fi
