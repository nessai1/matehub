#!/usr/bin/env bash
# pack-box-archive.sh — assemble a MateHub box-deploy tarball.
#
# Output layout (inside the tarball):
#
#   matehub-box-<variant>-<version>/
#     setup.sh                    # OS prep, docker install, .env bootstrap
#     up.sh                       # pull + compose up + healthcheck wait
#     down.sh                     # compose down (--purge for full wipe)
#     README.md                   # symlink-equivalent → README.en.md
#     README.en.md                # human instructions, English
#     README.ru.md                # human instructions, Russian
#     puller.json                 # YC SA pull-only credential
#     deploy/
#       docker-compose.yml        # renamed from docker-compose.box.yml
#       Caddyfile
#       .env.box.example          # versions pre-filled, manual fields blank
#       observability/
#         prometheus.box.yml
#         filebeat/filebeat.yml
#         grafana/...
#
# The three Linux variants (debian/ubuntu/rhel) ship identical content
# today — setup.sh detects the OS family at runtime. Variant suffix is
# kept for delivery / labelling so each tarball can be uploaded to a
# distinct release asset.
#
# Image versions are resolved from per-service git tags (`hub/X.Y.Z`,
# `chat/X.Y.Z`, `video/X.Y.Z`, `transcoder/X.Y.Z`). The latest tag for
# each service is substituted into .env.box.example so the archive ships
# pinned to a known-good image set.

set -euo pipefail

# ── Paths ────────────────────────────────────────────────────────────
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# ── Logging ──────────────────────────────────────────────────────────
ANSI_CYAN=$'\033[1;36m'
ANSI_GREEN=$'\033[1;32m'
ANSI_RED=$'\033[1;31m'
ANSI_RESET=$'\033[0m'
log() { printf '%s[pack]%s %s\n' "$ANSI_CYAN" "$ANSI_RESET" "$*"; }
ok()  { printf '%s[pack]%s %s\n' "$ANSI_GREEN" "$ANSI_RESET" "$*"; }
die() { printf '%s[pack] %s%s\n' "$ANSI_RED" "$*" "$ANSI_RESET" >&2; exit 1; }

# ── Args ─────────────────────────────────────────────────────────────
VARIANT=""
VERSION=""
# Defaults to the canonical local location. Override with --puller for
# CI runs that materialise the secret elsewhere (see release.yml).
PULLER="$REPO_ROOT/scripts/box/puller.json"
OUT_DIR="$REPO_ROOT/dist"

usage() {
    cat <<USAGE
Usage: $0 --variant VARIANT --version VERSION --puller PATH [--out DIR]

  --variant   debian | ubuntu | rhel
  --version   archive version, e.g. v0.0.1 (will appear in the tarball name)
  --puller    path to the pull-only YC SA key file (puller.json).
              Default: scripts/box/puller.json (gitignored, kept out of repo).
  --out       directory to write the tarball into (default: ./dist)

The image versions baked into .env.box.example are resolved from the
latest git tag for each service (hub/*, chat/*, video/*, transcoder/*).
Run from a clean checkout that has the relevant tags fetched.
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --variant) VARIANT="${2:-}"; shift 2;;
        --version) VERSION="${2:-}"; shift 2;;
        --puller)  PULLER="${2:-}";  shift 2;;
        --out)     OUT_DIR="${2:-}"; shift 2;;
        -h|--help) usage; exit 0;;
        *) die "unknown option: $1 (try --help)";;
    esac
done

[[ -n "$VARIANT" ]] || { usage; die "--variant is required"; }
[[ -n "$VERSION" ]] || { usage; die "--version is required"; }
[[ -n "$PULLER"  ]] || { usage; die "--puller is required"; }
case "$VARIANT" in
    debian|ubuntu|rhel) ;;
    *) die "invalid variant: $VARIANT (must be debian/ubuntu/rhel)";;
esac
[[ -f "$PULLER" ]] || die "puller key not found at $PULLER"

# ── Resolve service versions from latest tags ────────────────────────
latest_tag_for() {
    local prefix="$1"
    # Per-service tags are `<service>/<semver>`; sort by version descending.
    git -C "$REPO_ROOT" tag --list "${prefix}/*" --sort=-v:refname | head -n 1 \
        | sed -e "s|^${prefix}/||"
}

HUB_VER="$(latest_tag_for hub)"
CHAT_VER="$(latest_tag_for chat)"
VIDEO_VER="$(latest_tag_for video)"
TRANSCODER_VER="$(latest_tag_for transcoder)"

[[ -n "$HUB_VER"        ]] || die "no hub/* tags found -- push at least one image tag before packing"
[[ -n "$CHAT_VER"       ]] || die "no chat/* tags found"
[[ -n "$VIDEO_VER"      ]] || die "no video/* tags found"
[[ -n "$TRANSCODER_VER" ]] || die "no transcoder/* tags found"

log "resolved image versions:"
log "  hub        = $HUB_VER"
log "  chat       = $CHAT_VER"
log "  video      = $VIDEO_VER"
log "  transcoder = $TRANSCODER_VER"

# ── Stage layout ─────────────────────────────────────────────────────
ARCHIVE_NAME="matehub-box-${VARIANT}-${VERSION}"
STAGE="$(mktemp -d -t matehub-pack.XXXXXX)"
trap 'rm -rf "$STAGE"' EXIT
ROOT="$STAGE/$ARCHIVE_NAME"

mkdir -p "$ROOT/deploy/observability/grafana/provisioning/datasources"
mkdir -p "$ROOT/deploy/observability/grafana/provisioning/dashboards"
mkdir -p "$ROOT/deploy/observability/grafana/dashboards"
mkdir -p "$ROOT/deploy/observability/filebeat"

log "staging at $ROOT"

# ── Scripts ──────────────────────────────────────────────────────────
cp "$REPO_ROOT/scripts/box/setup.sh" "$ROOT/setup.sh"
cp "$REPO_ROOT/scripts/box/up.sh"    "$ROOT/up.sh"
cp "$REPO_ROOT/scripts/box/down.sh"  "$ROOT/down.sh"
chmod +x "$ROOT/setup.sh" "$ROOT/up.sh" "$ROOT/down.sh"

# ── READMEs ──────────────────────────────────────────────────────────
cp "$REPO_ROOT/scripts/box/README.en.md" "$ROOT/README.en.md"
cp "$REPO_ROOT/scripts/box/README.ru.md" "$ROOT/README.ru.md"
# README.md is the default landing read; English by convention.
# Tar archives don't preserve symlinks reliably across distro tar
# implementations, so we ship a copy.
cp "$REPO_ROOT/scripts/box/README.en.md" "$ROOT/README.md"

# ── Compose + Caddy + env ────────────────────────────────────────────
cp "$REPO_ROOT/deploy/docker-compose.box.yml" "$ROOT/deploy/docker-compose.yml"
cp "$REPO_ROOT/deploy/Caddyfile"               "$ROOT/deploy/Caddyfile"
cp "$REPO_ROOT/deploy/.env.box.example"        "$ROOT/deploy/.env.box.example"

# Bake resolved versions into the env template. setup.sh runs
# `cp .env.box.example .env`, so the operator gets pinned versions out
# of the box and can override them later if needed.
sed -i.bak "s|^HUB_VERSION=.*$|HUB_VERSION=${HUB_VER}|"               "$ROOT/deploy/.env.box.example"
sed -i.bak "s|^CHAT_VERSION=.*$|CHAT_VERSION=${CHAT_VER}|"            "$ROOT/deploy/.env.box.example"
sed -i.bak "s|^VIDEO_VERSION=.*$|VIDEO_VERSION=${VIDEO_VER}|"         "$ROOT/deploy/.env.box.example"
sed -i.bak "s|^TRANSCODER_VERSION=.*$|TRANSCODER_VERSION=${TRANSCODER_VER}|" "$ROOT/deploy/.env.box.example"
rm -f "$ROOT/deploy/.env.box.example.bak"

# ── Observability config ─────────────────────────────────────────────
cp "$REPO_ROOT/deploy/observability/prometheus.box.yml" \
   "$ROOT/deploy/observability/prometheus.box.yml"
cp "$REPO_ROOT/deploy/observability/filebeat/filebeat.yml" \
   "$ROOT/deploy/observability/filebeat/filebeat.yml"
cp -R "$REPO_ROOT/deploy/observability/grafana/provisioning/." \
      "$ROOT/deploy/observability/grafana/provisioning/"
# .gitkeep in dashboards/ is enough -- compose mounts the dir as a
# volume so empty is fine; future dashboard JSONs land here.
if [[ -f "$REPO_ROOT/deploy/observability/grafana/dashboards/.gitkeep" ]]; then
    cp "$REPO_ROOT/deploy/observability/grafana/dashboards/.gitkeep" \
       "$ROOT/deploy/observability/grafana/dashboards/.gitkeep"
fi

# ── Pull-only credential ─────────────────────────────────────────────
cp "$PULLER" "$ROOT/puller.json"
chmod 0600 "$ROOT/puller.json"

# ── Tarball ──────────────────────────────────────────────────────────
mkdir -p "$OUT_DIR"
TARBALL="$OUT_DIR/${ARCHIVE_NAME}.tar.gz"
log "creating $TARBALL"
tar -C "$STAGE" -czf "$TARBALL" "$ARCHIVE_NAME"

ok "wrote $TARBALL ($(du -h "$TARBALL" | cut -f1))"
