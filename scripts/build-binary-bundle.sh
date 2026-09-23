#!/usr/bin/env bash
#
# Builds the official binary bundle from the release binary already produced
# by the CI build job (BINPACKAGE.md sections 7-10).
#
# Usage:
#   scripts/build-binary-bundle.sh <version> [--binary <path>]
#
# Arguments:
#   <version>   release version, with or without the leading "v" (e.g. 0.3.0)
#   --binary    path to the built binary (default: target/release/gitilante)
#
# Output: dist/bin/gitilante-<version>-linux-x86_64.tar.gz containing
#
#   gitilante-<version>-linux-x86_64/
#     bin/gitilante
#     share/applications/dev.gitilante.Gitilante.desktop
#     share/icons/hicolor/scalable/apps/dev.gitilante.Gitilante.svg
#     share/metainfo/dev.gitilante.Gitilante.metainfo.xml
#
# The system libraries (GTK4, libadwaita, GtkSourceView, glibc) are never
# bundled: the binary uses the Arch system ones (BINPACKAGE.md section 9).

set -euo pipefail

log() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'EOF'
Usage: scripts/build-binary-bundle.sh <version> [--binary <path>]

Builds dist/bin/gitilante-<version>-linux-x86_64.tar.gz from the release
binary (default: target/release/gitilante). Does not build with Cargo.

Arguments:
  <version>   release version, with or without the leading "v" (e.g. 0.3.0)
  --binary    path to the built binary (default: target/release/gitilante)
EOF
}

VERSION=""
BINARY=""

while [ $# -gt 0 ]; do
    case "$1" in
        -h | --help)
            usage
            exit 0
            ;;
        --binary)
            shift
            [ $# -gt 0 ] || die "--binary requires a value"
            BINARY="$1"
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

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
BINARY="${BINARY:-target/release/gitilante}"
[ -f "$BINARY" ] || die "missing binary: $BINARY (run cargo build --release first)"

BUNDLE_NAME="gitilante-$VERSION-linux-x86_64"
OUT_DIR="dist/bin"
STAGE_DIR="$OUT_DIR/$BUNDLE_NAME"
ARCHIVE="$OUT_DIR/$BUNDLE_NAME.tar.gz"

log "Release version: $VERSION"
log "Binary: $BINARY"

# Binary verification before packaging (BINPACKAGE.md section 10).
log "Verifying the binary..."
FILE_TYPE="$(file -b "$BINARY")"
# PIE executables are the hardened default: accept both shapes.
case "$FILE_TYPE" in
    "ELF 64-bit LSB "*", x86-64"*)
        log "file: $FILE_TYPE"
        ;;
    *)
        die "unexpected binary type: $FILE_TYPE (expected ELF 64-bit x86-64)"
        ;;
esac

LDD_OUT="$(ldd "$BINARY")"
if printf '%s\n' "$LDD_OUT" | grep -q "not found"; then
    printf '%s\n' "$LDD_OUT" >&2
    die "the binary has unresolved shared libraries"
fi
log "ldd: all shared libraries resolved"

# An RPATH/RUNPATH would point at the build environment.
if readelf -d "$BINARY" | grep -qE "R(UN)?PATH"; then
    readelf -d "$BINARY" | grep -E "R(UN)?PATH" >&2
    die "the binary carries RPATH/RUNPATH entries"
fi
log "readelf: no RPATH/RUNPATH"

# The binary must not reference the CI build directories.
if grep -aq "/builds/" "$BINARY"; then
    die "the binary references CI build paths"
fi
log "no CI build paths referenced"

log "Staging $STAGE_DIR/..."
rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR/bin" \
    "$STAGE_DIR/share/applications" \
    "$STAGE_DIR/share/icons/hicolor/scalable/apps" \
    "$STAGE_DIR/share/metainfo"

install -Dm755 "$BINARY" "$STAGE_DIR/bin/gitilante"
install -Dm644 data/dev.gitilante.Gitilante.desktop \
    "$STAGE_DIR/share/applications/dev.gitilante.Gitilante.desktop"
install -Dm644 data/dev.gitilante.Gitilante.svg \
    "$STAGE_DIR/share/icons/hicolor/scalable/apps/dev.gitilante.Gitilante.svg"
install -Dm644 data/dev.gitilante.Gitilante.metainfo.xml \
    "$STAGE_DIR/share/metainfo/dev.gitilante.Gitilante.metainfo.xml"

log "Creating $ARCHIVE..."
mkdir -p "$OUT_DIR"
tar -czf "$ARCHIVE" -C "$OUT_DIR" "$BUNDLE_NAME"
rm -rf "$STAGE_DIR"

log "SHA256: $(sha256sum "$ARCHIVE" | cut -d' ' -f1)"
log "Done: $ARCHIVE"
