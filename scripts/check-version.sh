#!/usr/bin/env bash
#
# Verifies that a release tag matches the version declared in Cargo.toml.
#
# Usage:
#   scripts/check-version.sh [<tag>]
#
# The tag defaults to the CI_COMMIT_TAG environment variable. Fails when the
# two versions differ, printing a clear error message.

set -euo pipefail

TAG="${1:-${CI_COMMIT_TAG:-}}"

if [ -z "$TAG" ]; then
    echo "usage: scripts/check-version.sh [<tag>]  (or set CI_COMMIT_TAG)" >&2
    exit 2
fi

CARGO_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
if [ -z "$CARGO_VERSION" ]; then
    echo "error: no version found in Cargo.toml" >&2
    exit 1
fi

echo "Checking Cargo.toml version..."
if [ "v$CARGO_VERSION" != "$TAG" ]; then
    echo "Release tag $TAG does not match Cargo.toml version $CARGO_VERSION" >&2
    exit 1
fi

echo "Release version: $CARGO_VERSION"
echo "Tag: $TAG"
echo "Version check OK"
