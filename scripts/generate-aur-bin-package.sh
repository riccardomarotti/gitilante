#!/usr/bin/env bash
#
# Generates the AUR package metadata (PKGBUILD and .SRCINFO) for the binary
# package gitilante-bin (BINPACKAGE.md sections 18-19).
#
# Usage:
#   scripts/generate-aur-bin-package.sh <version> [--archive <tarball>] \
#       [--url <binary-url>] [--pkgrel <n>]
#
# Arguments:
#   <version>      release version, with or without the leading "v" (e.g. 0.3.0)
#   --archive      the binary bundle archive used to compute the real SHA-256
#                  (default: dist/bin/gitilante-<version>-linux-x86_64.tar.gz)
#   --url <url>    download URL of the bundle (default: the GitLab Generic
#                  Package Registry URL of this release)
#   --pkgrel <n>   package release number (default: 1; bump on packaging-only
#                  changes)
#
# Output: dist/aur-bin/PKGBUILD, dist/aur-bin/.SRCINFO

set -euo pipefail

PROJECT_URL="https://gitlab.com/rutilante/gitilante"
PROJECT_REF="rutilante%2Fgitilante"
PACKAGE_NAME="gitilante"
PKGBUILD_TEMPLATE="packaging/aur-bin/PKGBUILD.in"

log() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
Usage: scripts/generate-aur-bin-package.sh <version> [--archive <tarball>] [--url <binary-url>] [--pkgrel <n>]

Generates dist/aur-bin/PKGBUILD and dist/aur-bin/.SRCINFO for a release.

Arguments:
  <version>      release version, with or without the leading "v" (e.g. 0.3.0)
  --archive      the binary bundle archive used to compute the real SHA-256
                 (default: dist/bin/gitilante-<version>-linux-x86_64.tar.gz)
  --url <url>    download URL of the bundle (default: the GitLab Generic
                 Package Registry URL of this release)
  --pkgrel <n>   package release number (default: 1)
EOF
}

VERSION=""
PKGREL=1
ARCHIVE=""
BINARY_URL=""

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
        --url)
            shift
            [ $# -gt 0 ] || die "--url requires a value"
            BINARY_URL="$1"
            ;;
        --pkgrel)
            shift
            [ $# -gt 0 ] || die "--pkgrel requires a value"
            PKGREL="$1"
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
[[ "$PKGREL" =~ ^[0-9]+$ ]] || die "invalid package release: $PKGREL (expected a number)"
ARCHIVE="${ARCHIVE:-dist/bin/gitilante-$VERSION-linux-x86_64.tar.gz}"
[ -f "$ARCHIVE" ] || die "missing archive: $ARCHIVE"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
[ -f "$PKGBUILD_TEMPLATE" ] || die "missing template: $PKGBUILD_TEMPLATE"

OUT_DIR="dist/aur-bin"
FILE_NAME="gitilante-$VERSION-linux-x86_64.tar.gz"
BINARY_URL="${BINARY_URL:-https://gitlab.com/api/v4/projects/$PROJECT_REF/packages/generic/$PACKAGE_NAME/$VERSION/$FILE_NAME}"

mkdir -p "$OUT_DIR"

log "Release version: $VERSION"
log "Package release: $PKGREL"
log "Binary bundle: $ARCHIVE"
log "Binary URL: $BINARY_URL"

SHA256="$(sha256sum "$ARCHIVE" | cut -d' ' -f1)"
log "SHA256: $SHA256"

log "Generating PKGBUILD..."
sed -e "s|@VERSION@|$VERSION|g" \
    -e "s|@PKGREL@|$PKGREL|g" \
    -e "s|@BINARY_URL@|$BINARY_URL|g" \
    -e "s|@SHA256@|$SHA256|g" \
    "$PKGBUILD_TEMPLATE" >"$OUT_DIR/PKGBUILD"
if grep -q '@VERSION@\|@PKGREL@\|@BINARY_URL@\|@SHA256@' "$OUT_DIR/PKGBUILD"; then
    die "unreplaced placeholders left in $OUT_DIR/PKGBUILD"
fi

log "Generating .SRCINFO..."
# makepkg reads the PKGBUILD in the current directory.
(
    cd "$OUT_DIR"
    makepkg --printsrcinfo >.SRCINFO
) || die "makepkg --printsrcinfo failed"

log "Done: $OUT_DIR/PKGBUILD, $OUT_DIR/.SRCINFO"
