#!/usr/bin/env bash
set -euo pipefail

# Usage: ./scripts/version-bump.sh <new-version>
# Example: ./scripts/version-bump.sh 2026.3.19
#
# Updates the version in all locations:
#   - VERSION (source of truth, read by CI/CD)
#   - Cargo.toml package version
#
# After running, commit and tag:
#   git add VERSION Cargo.toml Cargo.lock
#   git commit -m "bump to $VERSION"
#   git tag $VERSION && git push && git push --tags

if [ $# -ne 1 ]; then
    echo "Usage: $0 <new-version>"
    echo "Example: $0 2026.3.19"
    exit 1
fi

NEW_VERSION="$1"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Read current version
OLD_VERSION="$(cat "$ROOT/VERSION" | tr -d '[:space:]')"

if [ "$OLD_VERSION" = "$NEW_VERSION" ]; then
    echo "Already at version $NEW_VERSION"
    exit 0
fi

echo "Bumping $OLD_VERSION -> $NEW_VERSION"

# 1. VERSION file (source of truth for CI/CD)
printf '%s\n' "$NEW_VERSION" > "$ROOT/VERSION"
echo "  updated VERSION"

# 2. Cargo.toml package version
sed -i "s/^version = \"$OLD_VERSION\"/version = \"$NEW_VERSION\"/" "$ROOT/Cargo.toml"
echo "  updated Cargo.toml"

# 3. Regenerate Cargo.lock
cd "$ROOT"
cargo generate-lockfile --quiet 2>/dev/null || true
echo "  updated Cargo.lock"

# Verify consistency
CARGO_VER="$(grep '^version = ' "$ROOT/Cargo.toml" | head -1 | sed 's/version = "\(.*\)"/\1/')"
FILE_VER="$(cat "$ROOT/VERSION" | tr -d '[:space:]')"

if [ "$CARGO_VER" != "$NEW_VERSION" ]; then
    echo "ERROR: Cargo.toml has '$CARGO_VER', expected '$NEW_VERSION'"
    exit 1
fi
if [ "$FILE_VER" != "$NEW_VERSION" ]; then
    echo "ERROR: VERSION has '$FILE_VER', expected '$NEW_VERSION'"
    exit 1
fi

echo ""
echo "Version bumped to $NEW_VERSION"
echo ""
echo "Next steps:"
echo "  git add VERSION Cargo.toml Cargo.lock"
echo "  git commit -m \"bump to $NEW_VERSION\""
echo "  git tag $NEW_VERSION"
echo "  git push && git push --tags"
