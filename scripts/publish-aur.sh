#!/usr/bin/env bash
#
# Publishes the generated AUR package metadata to the AUR.
#
# Usage:
#   scripts/publish-aur.sh
#
# Expects dist/aur/PKGBUILD and dist/aur/.SRCINFO (from
# scripts/generate-aur-package.sh) and publishes them to the AUR package in
# AUR_PACKAGE. The update is idempotent: when the AUR repository already has
# exactly those files, nothing is committed or pushed.
#
# Environment:
#   AUR_SSH_PRIVATE_KEY   base64-encoded SSH private key dedicated to the AUR
#                         (used by CI; when unset, the ambient SSH agent and
#                         configuration are used, so the script also works
#                         locally)
#   AUR_PACKAGE           AUR package name (default: gitilante)
#   AUR_GIT_USER_NAME     author name for the AUR commit
#   AUR_GIT_USER_EMAIL    author email for the AUR commit
#
# Security (DEPLOY.md section 13): the private key is decoded into a private
# temporary directory, never printed, and removed on exit; SSH verifies the
# host against the pinned keys in packaging/aur/known_hosts.

set -euo pipefail

AUR_PACKAGE="${AUR_PACKAGE:-gitilante}"
AUR_URL="ssh://aur@aur.archlinux.org/$AUR_PACKAGE.git"
AUTHOR_NAME="${AUR_GIT_USER_NAME:-Riccardo Marotti}"
AUTHOR_EMAIL="${AUR_GIT_USER_EMAIL:-riccardo.marotti@gmail.com}"

log() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

METADATA_DIR="dist/aur"
[ -f "$METADATA_DIR/PKGBUILD" ] || die "missing $METADATA_DIR/PKGBUILD (run scripts/generate-aur-package.sh first)"
[ -f "$METADATA_DIR/.SRCINFO" ] || die "missing $METADATA_DIR/.SRCINFO (run scripts/generate-aur-package.sh first)"
[ -f "packaging/aur/known_hosts" ] || die "missing packaging/aur/known_hosts"

# The commit message reports the packaged version (pkgver, or pkgver-pkgrel
# when this is not the first package release).
PKGVER="$(sed -n 's/^pkgver=//p' "$METADATA_DIR/PKGBUILD" | head -1)"
PKGREL="$(sed -n 's/^pkgrel=//p' "$METADATA_DIR/PKGBUILD" | head -1)"
[ -n "$PKGVER" ] || die "no pkgver in $METADATA_DIR/PKGBUILD"
[ -n "$PKGREL" ] || die "no pkgrel in $METADATA_DIR/PKGBUILD"
UPDATE="$PKGVER"
[ "$PKGREL" = "1" ] || UPDATE="$PKGVER-$PKGREL"

log "Publishing AUR package: $AUR_PACKAGE $UPDATE"

SSH_DIR=""
cleanup() {
    # Never leave key material behind.
    [ -z "$SSH_DIR" ] || rm -rf "$SSH_DIR"
}
trap cleanup EXIT

if [ -n "${AUR_SSH_PRIVATE_KEY:-}" ]; then
    # CI: the key lives in a protected masked variable as a single base64
    # line. Decode it into a private temporary file and never print it.
    SSH_DIR="$(mktemp -d)"
    chmod 700 "$SSH_DIR"
    umask 077
    printf '%s' "$AUR_SSH_PRIVATE_KEY" | base64 -d >"$SSH_DIR/key" || die "cannot decode AUR_SSH_PRIVATE_KEY"
    chmod 600 "$SSH_DIR/key"
    export GIT_SSH_COMMAND="ssh -i '$SSH_DIR/key' -o IdentitiesOnly=yes -o UserKnownHostsFile='$ROOT/packaging/aur/known_hosts'"
else
    log "Using the ambient SSH configuration (local run)"
fi

WORK_DIR="$(mktemp -d)"
cleanup_work() {
    cleanup
    rm -rf "$WORK_DIR"
}
trap cleanup_work EXIT

log "Cloning AUR repository..."
git clone --quiet "$AUR_URL" "$WORK_DIR/$AUR_PACKAGE"
cd "$WORK_DIR/$AUR_PACKAGE"

cp "$ROOT/$METADATA_DIR/PKGBUILD" "$ROOT/$METADATA_DIR/.SRCINFO" .

if [ -z "$(git status --porcelain)" ]; then
    log "AUR package is already up to date ($UPDATE): nothing to publish"
    exit 0
fi

git add PKGBUILD .SRCINFO
git -c user.name="$AUTHOR_NAME" -c user.email="$AUTHOR_EMAIL" commit -qm "Update to $UPDATE"
git push --quiet

log "Published $AUR_PACKAGE $UPDATE to the AUR"
