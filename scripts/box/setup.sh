#!/usr/bin/env bash
# MateHub box deploy -- setup phase.
#
# This is the script bundled inside the box archive. It does the
# one-time host preparation:
#   1. Detect the OS family (debian/ubuntu/rhel).
#   2. Install Docker Engine + Compose plugin if missing.
#   3. Add the invoking user to the `docker` group.
#   4. `docker login` against the configured registry using `puller.json`.
#   5. Ask whether S3 should be the bundled MinIO or an external bucket.
#   6. Ask whether to enable the observability profile (Grafana + Kibana
#      + Filebeat + Prometheus + Elasticsearch).
#   7. Materialise `deploy/.env` from the template, generating any blank
#      secrets via `openssl rand`. Existing values are never overwritten.
#   8. Prompt for DOMAIN / PUBLIC_IP / ACME_EMAIL (and external S3 fields
#      if that path was chosen).
#   9. Print a loud red warning about DNS having to resolve before `up`.
#
# Idempotent: re-running after a successful setup is a no-op except for
# unfilled fields. Existing secrets stay; existing manual fields stay.
#
# Designed to work without already-installed `jq`, `curl`'s extras, or any
# third-party CLIs beyond Docker / openssl. The only tools assumed are
# what every Linux distro ships in `coreutils` + `bash` 4+.

set -euo pipefail

# ── Layout ───────────────────────────────────────────────────────────
BUNDLE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEPLOY_DIR="$BUNDLE_ROOT/deploy"
ENV_FILE="$DEPLOY_DIR/.env"
ENV_TEMPLATE="$DEPLOY_DIR/.env.box.example"
PULLER_FILE="$BUNDLE_ROOT/puller.json"

# ── Logging helpers ──────────────────────────────────────────────────
ANSI_CYAN=$'\033[1;36m'
ANSI_GREEN=$'\033[1;32m'
ANSI_YELLOW=$'\033[1;33m'
ANSI_RED=$'\033[1;31m'
ANSI_RESET=$'\033[0m'

log()  { printf '%s[setup]%s %s\n'   "$ANSI_CYAN"   "$ANSI_RESET" "$*"; }
ok()   { printf '%s[setup]%s %s\n'   "$ANSI_GREEN"  "$ANSI_RESET" "$*"; }
warn() { printf '%s[setup]%s %s\n'   "$ANSI_YELLOW" "$ANSI_RESET" "$*" >&2; }
die()  { printf '%s[setup] %s%s\n'   "$ANSI_RED"    "$*" "$ANSI_RESET" >&2; exit 1; }
red_block() {
    printf '\n%s================================================================%s\n' "$ANSI_RED" "$ANSI_RESET"
    while IFS= read -r line; do
        printf '%s    %s%s\n' "$ANSI_RED" "$line" "$ANSI_RESET"
    done <<<"$1"
    printf '%s================================================================%s\n\n' "$ANSI_RED" "$ANSI_RESET"
}

# ── Argument parsing ─────────────────────────────────────────────────
DOMAIN_ARG=""
PUBLIC_IP_ARG=""
ACME_EMAIL_ARG=""
S3_MODE_ARG=""
OBSERVABILITY_ARG=""
UNATTENDED=0

usage() {
    cat <<USAGE
Usage: $0 [options]

Options:
  --domain DOMAIN              Public domain (DNS A-record must already point here).
  --public-ip IP               IPv4 address of this host.
  --acme-email EMAIL           Email used by Let's Encrypt for renewal notices.
  --s3-mode local|external     S3 backend choice. Default: prompt.
  --enable-observability       Turn on the Grafana / Kibana stack.
  --no-observability           Skip the observability stack.
  --unattended                 Fail if any required value is missing instead of prompting.
  -h, --help                   Show this help.
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --domain)              DOMAIN_ARG="${2:-}";       shift 2;;
        --public-ip)           PUBLIC_IP_ARG="${2:-}";    shift 2;;
        --acme-email)          ACME_EMAIL_ARG="${2:-}";   shift 2;;
        --s3-mode)             S3_MODE_ARG="${2:-}";      shift 2;;
        --enable-observability) OBSERVABILITY_ARG="yes";   shift;;
        --no-observability)    OBSERVABILITY_ARG="no";    shift;;
        --unattended)          UNATTENDED=1;              shift;;
        -h|--help)             usage; exit 0;;
        *) die "unknown option: $1 (try --help)";;
    esac
done

prompt_or_die() {
    local prompt="$1" preset="${2:-}"
    if [[ -n "$preset" ]]; then echo "$preset"; return; fi
    if [[ "$UNATTENDED" -eq 1 ]]; then die "$prompt is required in --unattended mode"; fi
    local answer
    read -r -p "$prompt: " answer
    echo "$answer"
}

prompt_yes_no() {
    local q="$1" preset="${2:-}"
    if [[ "$preset" == "yes" ]]; then echo "yes"; return; fi
    if [[ "$preset" == "no"  ]]; then echo "no";  return; fi
    if [[ "$UNATTENDED" -eq 1 ]]; then die "$q must be set explicitly in --unattended"; fi
    local answer
    while true; do
        read -r -p "$q [y/N]: " answer
        case "${answer,,}" in
            y|yes) echo "yes"; return;;
            ""|n|no) echo "no"; return;;
        esac
    done
}

# ── 1. OS detection ──────────────────────────────────────────────────
detect_os_family() {
    if [[ -f /etc/os-release ]]; then
        # shellcheck disable=SC1091
        . /etc/os-release
        case "${ID_LIKE:-} ${ID:-}" in
            *debian*|*ubuntu*) echo "debian"; return;;
            *rhel*|*centos*|*fedora*|*rocky*|*almalinux*) echo "rhel"; return;;
        esac
        case "${ID:-}" in
            debian|ubuntu) echo "debian"; return;;
            rhel|centos|fedora|rocky|almalinux) echo "rhel"; return;;
        esac
    fi
    die "unsupported OS: cannot detect family from /etc/os-release"
}

# ── 2. Docker install ────────────────────────────────────────────────
ensure_docker() {
    if command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then
        ok "docker + compose plugin already installed"
        return
    fi
    log "installing docker (this may take a couple of minutes)"
    if ! command -v curl >/dev/null 2>&1; then
        local family
        family="$(detect_os_family)"
        case "$family" in
            debian) sudo apt-get update -qq && sudo apt-get install -y curl;;
            rhel)   sudo dnf install -y curl;;
        esac
    fi
    # Docker's convenience script handles all three families and
    # picks the right repo per distro. Widely vetted; rolling our own
    # apt/dnf flow doubles maintenance for zero gain.
    curl -fsSL https://get.docker.com | sudo sh
    sudo systemctl enable --now docker
    ok "docker installed"
}

# ── 3. Docker group membership ───────────────────────────────────────
ensure_docker_group() {
    local user="${USER:-$(id -un)}"
    if id -nG "$user" | tr ' ' '\n' | grep -qx docker; then
        ok "user '$user' already in 'docker' group"
        return
    fi
    log "adding user '$user' to 'docker' group"
    sudo usermod -aG docker "$user"
    warn "user '$user' added to docker group -- log out and log back in for the change to take effect, then re-run setup.sh"
    warn "(this is required by Docker, not by us; the new gid only attaches on a fresh login session)"
    exit 0
}

# ── 4. Registry login ────────────────────────────────────────────────
ensure_registry_login() {
    [[ -f "$PULLER_FILE" ]] || die "puller.json missing at $PULLER_FILE -- the archive is incomplete"
    if [[ -f "$HOME/.docker/config.json" ]] \
       && grep -q '"cr.yandex"' "$HOME/.docker/config.json" 2>/dev/null; then
        ok "already authenticated with cr.yandex"
        return
    fi
    log "logging in to cr.yandex via puller.json"
    cat "$PULLER_FILE" | docker login --username json_key --password-stdin cr.yandex
    ok "registry login succeeded"
}

# ── 5/6. Mode prompts ────────────────────────────────────────────────
choose_s3_mode() {
    local mode="$S3_MODE_ARG"
    if [[ -z "$mode" ]]; then
        if [[ "$UNATTENDED" -eq 1 ]]; then
            mode="local"
        else
            echo
            echo "  S3 backend:"
            echo "    [1] local     -- bundled MinIO (everything self-contained)"
            echo "    [2] external  -- AWS / Yandex Cloud / your own S3-compatible store"
            local choice
            read -r -p "  Choose [1]: " choice
            case "${choice:-1}" in
                1) mode="local";;
                2) mode="external";;
                *) die "invalid choice: $choice";;
            esac
        fi
    fi
    [[ "$mode" == "local" || "$mode" == "external" ]] || die "invalid --s3-mode: $mode"
    echo "$mode"
}

choose_observability() {
    prompt_yes_no "  Enable observability stack (Grafana + Kibana + Prometheus + Elasticsearch)?" "$OBSERVABILITY_ARG"
}

# ── 7. .env materialisation ──────────────────────────────────────────
gen_secret() { openssl rand -hex 32; }

set_env_blank() {
    # Set KEY=value in $ENV_FILE only if the existing value is empty.
    # Preserves comments and ordering.
    local key="$1" value="$2"
    if grep -qE "^${key}=$" "$ENV_FILE"; then
        sed -i.bak "s|^${key}=$|${key}=${value}|" "$ENV_FILE"
        rm -f "$ENV_FILE.bak"
        log "generated $key"
    fi
}

set_env_value() {
    # Set KEY=value unconditionally (creates the line if missing).
    local key="$1" value="$2"
    if grep -qE "^${key}=" "$ENV_FILE"; then
        sed -i.bak "s|^${key}=.*|${key}=${value}|" "$ENV_FILE"
        rm -f "$ENV_FILE.bak"
    else
        echo "${key}=${value}" >> "$ENV_FILE"
    fi
}

bcrypt_hash() {
    # Generate a bcrypt hash via Caddy itself -- we already pull
    # caddy:2-alpine for the stack, so no extra image. Output is a single
    # bcrypt hash on stdout.
    docker run --rm caddy:2-alpine caddy hash-password --plaintext "$1"
}

# ── 8. Manual fields ─────────────────────────────────────────────────
collect_manual_fields() {
    local domain public_ip acme_email
    domain="$(prompt_or_die "  Domain (DNS A-record must already point here)" "$DOMAIN_ARG")"
    public_ip="$(prompt_or_die "  Public IPv4 of this host" "$PUBLIC_IP_ARG")"
    acme_email="$(prompt_or_die "  ACME email (Let's Encrypt notifications)" "$ACME_EMAIL_ARG")"
    [[ -n "$domain"     ]] || die "DOMAIN is required"
    [[ -n "$public_ip"  ]] || die "PUBLIC_IP is required"
    [[ -n "$acme_email" ]] || die "ACME_EMAIL is required"
    set_env_value DOMAIN     "$domain"
    set_env_value PUBLIC_IP  "$public_ip"
    set_env_value ACME_EMAIL "$acme_email"
}

ensure_external_s3_placeholders() {
    log "external S3 selected -- the relevant fields are LEFT BLANK in .env."
    log "fill them in before running ./up.sh:"
    log "    S3_ENDPOINT, S3_REGION, S3_PUBLIC_URL,"
    log "    S3_ACCESS_KEY_ID, S3_SECRET_ACCESS_KEY,"
    log "    S3_BUCKET_HUB_ASSETS, S3_BUCKET_CHAT_MEDIA"
    for k in S3_ENDPOINT S3_REGION S3_PUBLIC_URL \
             S3_ACCESS_KEY_ID S3_SECRET_ACCESS_KEY \
             S3_BUCKET_HUB_ASSETS S3_BUCKET_CHAT_MEDIA; do
        if ! grep -qE "^${k}=" "$ENV_FILE"; then
            echo "${k}=" >> "$ENV_FILE"
        fi
    done
}

# ── Main ─────────────────────────────────────────────────────────────
main() {
    [[ "$(uname -s)" == "Linux" ]] || die "setup.sh runs on Linux only ($(uname -s) detected)"

    log "starting MateHub box setup"

    ensure_docker
    ensure_docker_group
    ensure_registry_login

    [[ -f "$ENV_TEMPLATE" ]] || die "missing $ENV_TEMPLATE in archive"
    if [[ ! -f "$ENV_FILE" ]]; then
        cp "$ENV_TEMPLATE" "$ENV_FILE"
        ok "created $ENV_FILE from template"
    else
        log "$ENV_FILE already exists -- only blank fields will be filled"
    fi

    local s3_mode
    s3_mode="$(choose_s3_mode)"
    log "S3 mode: $s3_mode"

    local observability
    observability="$(choose_observability)"
    log "observability: $observability"

    collect_manual_fields

    set_env_blank POSTGRES_PASSWORD "$(gen_secret)"
    set_env_blank JWT_SECRET        "$(gen_secret)"
    set_env_blank S3_SECRET_KEY     "$(gen_secret)"
    set_env_blank TURN_PASSWORD     "$(gen_secret)"

    if [[ "$s3_mode" == "external" ]]; then
        ensure_external_s3_placeholders
    fi

    if [[ "$observability" == "yes" ]]; then
        local obs_pwd obs_hash gf_pwd
        obs_pwd="$(gen_secret)"
        log "hashing observability password (one-shot caddy run)"
        obs_hash="$(bcrypt_hash "$obs_pwd")"
        gf_pwd="$(gen_secret)"
        set_env_value OBSERVABILITY_USER          "admin"
        set_env_value OBSERVABILITY_PASSWORD      "$obs_pwd"
        set_env_value OBSERVABILITY_PASSWORD_HASH "$obs_hash"
        set_env_value GRAFANA_ADMIN_USER          "admin"
        set_env_value GRAFANA_ADMIN_PASSWORD      "$gf_pwd"
        # Persist the choice so up.sh knows whether to add --profile observability.
        set_env_value MATEHUB_OBSERVABILITY       "1"
    else
        set_env_value MATEHUB_OBSERVABILITY       "0"
    fi

    ok "setup complete"
    echo

    red_block "$(cat <<'WARN'
BEFORE RUNNING ./up.sh:

  1. Make sure the DNS A-record for your DOMAIN points at PUBLIC_IP.
     Verify with:   dig +short YOUR.DOMAIN
     If it returns an empty/wrong answer, do NOT run ./up.sh yet --
     Caddy will hit Let's Encrypt with the wrong host, fail the ACME
     HTTP-01 challenge, and Let's Encrypt rate-limits will lock you
     out of certificate issuance for up to an hour.

  2. Open ports 80/tcp, 443/tcp, 4001/udp, 3478/udp+tcp,
     49152-65535/udp on the host firewall and any cloud security group.
WARN
)"

    if [[ "$s3_mode" == "external" ]]; then
        red_block "EXTERNAL S3 SELECTED. Edit deploy/.env and fill in:
  S3_ENDPOINT, S3_REGION, S3_PUBLIC_URL,
  S3_ACCESS_KEY_ID, S3_SECRET_ACCESS_KEY,
  S3_BUCKET_HUB_ASSETS, S3_BUCKET_CHAT_MEDIA
The bundled MinIO will not start in this mode."
    fi

    log "next: ./up.sh"
}

main "$@"
