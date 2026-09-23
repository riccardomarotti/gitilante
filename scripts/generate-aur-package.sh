#!/usr/bin/env bash
#
# Generates the AUR package metadata (PKGBUILD and .SRCINFO) for a release.
#
# Usage:
#   scripts/generate-aur-package.sh <version>           # from the GitLab tag archive
#   scripts/generate-aur-package.sh <version> --local   # local git archive (testing only)
#
# Arguments:
#   <version>   release version, with or without the leading "v" (e.g. 0.2.0)
#   --local     build the source archive from the local repository instead of
#               downloading the GitLab tag archive. The checksum then refers to
#               the local archive: the result is for testing only and must not
#               be published to the AUR.
#   --ref <r>   git ref for --local (default: HEAD)
#
# Output: dist/aur/PKGBUILD, dist/aur/.SRCINFO (and the source archive).

set -euo pipefail

PROJECT_URL="https://gitlab.com/rutilante/gitilante"
PKGBUILD_TEMPLATE="packaging/aur/PKGBUILD.in"

log() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
Usage: scripts/generate-aur-package.sh <version> [--local [--ref <ref>]]

Generates dist/aur/PKGBUILD and dist/aur/.SRCINFO for a release.

Arguments:
  <version>   release version, with or without the leading "v" (e.g. 0.2.0)
  --pkgrel N  package release number (default: 1; bump on packaging-only changes)
  --local     build the source archive from the local repository instead of
              downloading the GitLab tag archive. The checksum then refers to
              the local archive: the result is for testing only and must not
              be published to the AUR.
  --ref <r>   git ref for --local (default: HEAD)
EOF
}

VERSION=""
PKGREL=1
LOCAL=0
REF="HEAD"

while [ $# -gt 0 ]; do
    case "$1" in
        -h | --help)
            usage
            exit 0
            ;;
        --local)
            LOCAL=1
            ;;
        --ref)
            shift
            [ $# -gt 0 ] || die "--ref requires a value"
            REF="$1"
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

# Tolerate a tag-shaped input; the version is always without the leading "v".
VERSION="${VERSION#v}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "invalid version: $VERSION (expected MAJOR.MINOR.PATCH)"
[[ "$PKGREL" =~ ^[0-9]+$ ]] || die "invalid package release: $PKGREL (expected a number)"
TAG="v$VERSION"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
[ -f "$PKGBUILD_TEMPLATE" ] || die "missing template: $PKGBUILD_TEMPLATE"

OUT_DIR="dist/aur"
# Local filename of the source archive (see the `source` rename in the template).
ARCHIVE_NAME="gitilante-$VERSION.tar.gz"
# Top level directory of the archive produced for the tag.
EXTRACTED_DIR="gitilante-v$VERSION"
SOURCE_URL="$PROJECT_URL/-/archive/$TAG/$EXTRACTED_DIR.tar.gz"

mkdir -p "$OUT_DIR"

log "Release version: $VERSION"
log "Package release: $PKGREL"
log "Tag: $TAG"

if [ "$LOCAL" -eq 1 ]; then
    log "Source archive: local git archive of $REF (TESTING ONLY, do not publish)"
    git archive --format=tar.gz --prefix="$EXTRACTED_DIR/" -o "$OUT_DIR/$ARCHIVE_NAME" "$REF"
else
    log "Source archive: $SOURCE_URL"
    curl --fail --silent --show-error --location --output "$OUT_DIR/$ARCHIVE_NAME" "$SOURCE_URL"
fi

SHA256="$(sha256sum "$OUT_DIR/$ARCHIVE_NAME" | cut -d' ' -f1)"
log "SHA256: $SHA256"

log "Generating PKGBUILD..."
sed -e "s/@VERSION@/$VERSION/g" -e "s/@PKGREL@/$PKGREL/g" -e "s/@SHA256@/$SHA256/g" "$PKGBUILD_TEMPLATE" >"$OUT_DIR/PKGBUILD"
if grep -q '@VERSION@\|@PKGREL@\|@SHA256@' "$OUT_DIR/PKGBUILD"; then
    die "unreplaced placeholders left in $OUT_DIR/PKGBUILD"
fi

log "Generating .SRCINFO..."
# makepkg reads the PKGBUILD in the current directory.
(
    cd "$OUT_DIR"
    makepkg --printsrcinfo >.SRCINFO
) || die "makepkg --printsrcinfo failed"

log "Done: $OUT_DIR/PKGBUILD, $OUT_DIR/.SRCINFO"
