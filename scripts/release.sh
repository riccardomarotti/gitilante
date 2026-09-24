#!/usr/bin/env bash
#
# Prepares and publishes a Gitilante release.
#
# Usage:
#   scripts/release.sh <version>
#
# Runs the whole release flow:
#   - updates the package version in Cargo.toml and refreshes Cargo.lock;
#   - refreshes the release list in the AppStream metainfo;
#   - runs the test suite and the version check;
#   - commits "Prepare release <version>" and creates the annotated tag;
#   - pushes the commit and the tag.
#
# Fails when the working tree is dirty: a release only contains release
# changes.

set -euo pipefail

cd "$(dirname "$0")/.."

VERSION="${1:-}"

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "usage: scripts/release.sh <version>   (e.g. 0.8.0)" >&2
    exit 2
fi

TAG="v$VERSION"

if [ -n "$(git status --porcelain)" ]; then
    echo "error: the working tree is not clean; commit or stash first" >&2
    exit 1
fi

if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
    echo "error: tag $TAG already exists" >&2
    exit 1
fi

echo "==> Updating Cargo.toml to $VERSION"
sed -i "0,/^version = \"[0-9]/s/^version = \"[0-9][^\"]*\"/version = \"$VERSION\"/" Cargo.toml
cargo check --quiet

echo "==> Refreshing the AppStream release list"
RELEASE_VERSION="$VERSION" python3 - <<'PYEOF'
import os
import re
import subprocess
from datetime import date
from pathlib import Path

version = os.environ["RELEASE_VERSION"]
tags = subprocess.run(
    ["git", "for-each-ref", "refs/tags", "--format=%(refname:short) %(creatordate:short)"],
    capture_output=True,
    text=True,
    check=True,
).stdout.split()
pairs = list(zip(tags[0::2], tags[1::2])) + [("v" + version, date.today().isoformat())]
pairs.sort(key=lambda pair: tuple(int(part) for part in pair[0][1:].split(".")), reverse=True)
entries = "\n".join(
    f'    <release version="{tag[1:]}" date="{when}"/>' for tag, when in pairs
)

path = Path("data/dev.gitilante.Gitilante.metainfo.xml")
text = path.read_text()
text = re.sub(
    r"  <releases>.*?</releases>",
    f"  <releases>\n{entries}\n  </releases>",
    text,
    flags=re.S,
)
path.write_text(text)
PYEOF

echo "==> Running the test suite"
cargo test --quiet 2>&1 | grep "test result:"

echo "==> Checking the version"
scripts/check-version.sh "$TAG"

echo "==> Committing and tagging"
git add Cargo.toml Cargo.lock data/dev.gitilante.Gitilante.metainfo.xml
git commit -qm "Prepare release $VERSION"
git tag -a "$TAG" -m "Gitilante $VERSION"

echo "==> Pushing"
git push
git push origin "$TAG"

echo "Release $VERSION published."
