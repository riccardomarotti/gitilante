#!/usr/bin/env bash
#
# Publishes the official binary bundle to the GitLab Generic Package Registry
# (BINPACKAGE.md sections 11-13 and 31).
#
# Usage:
#   scripts/publish-binary-bundle.sh <version> [--archive <path>]
#
# Arguments:
#   <version>   release version, with or without the leading "v" (e.g. 0.3.0)
#   --archive   path to the bundle archive
#               (default: dist/bin/gitilante-<version>-linux-x86_64.tar.gz)
#
# Environment:
#   CI_JOB_TOKEN    GitLab job token used for the upload (required)
#   CI_API_V4_URL   GitLab API base URL (default: https://gitlab.com/api/v4)
#   CI_PROJECT_ID   numeric project id or URL-encoded path
#                   (default: rutilante%2Fgitilante)
#
# Release immutability (BINPACKAGE.md section 31):
#   asset missing            -> upload
#   asset present, same hash -> OK (idempotent)
#   asset present, different -> FAIL

set -euo pipefail

PACKAGE_NAME="gitilante"

log() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
Usage: scripts/publish-binary-bundle.sh <version> [--archive <path>]

Publishes the binary bundle to the GitLab Generic Package Registry.

Arguments:
  <version>   release version, with or without the leading "v" (e.g. 0.3.0)
  --archive   path to the bundle archive
              (default: dist/bin/gitilante-<version>-linux-x86_64.tar.gz)
EOF
}

VERSION=""
ARCHIVE=""

while [ $# -gt 0 ]; do
    case "$1" in
        -h | --help)
            usage
            exit 0
            ;;
        --archive)
            shift
            [ $# -gt 0 ] || die "--archive requires a value"
            ARCHIVE="$1"
            ;;
        -*)
            die "unknown option: $1"
            ;;
        *)
            [ -z "$VERSION" ] || die "unexpected argument: $1"
            VERSION="$1"
            ;;
    esac
    shift
done

[ -n "$VERSION" ] || { usage >&2; exit 2; }
VERSION="${VERSION#v}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "invalid version: $VERSION (expected MAJOR.MINOR.PATCH)"
[ -n "${CI_JOB_TOKEN:-}" ] || die "CI_JOB_TOKEN is not set (upload requires a GitLab job token)"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

FILE_NAME="gitilante-$VERSION-linux-x86_64.tar.gz"
ARCHIVE="${ARCHIVE:-dist/bin/$FILE_NAME}"
[ -f "$ARCHIVE" ] || die "missing archive: $ARCHIVE (run scripts/build-binary-bundle.sh first)"

API_URL="${CI_API_V4_URL:-https://gitlab.com/api/v4}"
PROJECT_REF="${CI_PROJECT_ID:-rutilante%2Fgitilante}"
PACKAGE_URL="$API_URL/projects/$PROJECT_REF/packages/generic/$PACKAGE_NAME/$VERSION/$FILE_NAME"

LOCAL_SHA256="$(sha256sum "$ARCHIVE" | cut -d' ' -f1)"

log "Release version: $VERSION"
log "Archive: $ARCHIVE"
log "SHA256: $LOCAL_SHA256"
log "Registry URL: $PACKAGE_URL"

REMOTE_FILE="$(mktemp)"
trap 'rm -f "$REMOTE_FILE"' EXIT

HTTP_CODE="$(curl --silent --show-error --location \
    --header "JOB-TOKEN: $CI_JOB_TOKEN" \
    --output "$REMOTE_FILE" --write-out '%{http_code}' \
    "$PACKAGE_URL" || true)"

case "$HTTP_CODE" in
    200)
        REMOTE_SHA256="$(sha256sum "$REMOTE_FILE" | cut -d' ' -f1)"
        if [ "$REMOTE_SHA256" != "$LOCAL_SHA256" ]; then
            die "a different $FILE_NAME is already published (remote sha256 $REMOTE_SHA256): releases are immutable"
        fi
        log "Already published with identical content: nothing to upload"
        exit 0
        ;;
    404)
        ;;
    *)
        die "unexpected HTTP $HTTP_CODE while checking $PACKAGE_URL"
        ;;
esac

log "Uploading $ARCHIVE..."
curl --fail --silent --show-error --location \
    --header "JOB-TOKEN: $CI_JOB_TOKEN" \
    --upload-file "$ARCHIVE" \
    "$PACKAGE_URL" >/dev/null

log "Published $FILE_NAME to the GitLab Package Registry"
