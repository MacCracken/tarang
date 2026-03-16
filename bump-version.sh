#!/bin/bash
set -euo pipefail

if [ $# -ne 1 ]; then
    echo "Usage: $0 <new-version>"
    echo "Example: $0 2026.3.17"
    exit 1
fi

NEW_VERSION="$1"
OLD_VERSION=$(cat VERSION | tr -d '[:space:]')

echo "Bumping version: ${OLD_VERSION} -> ${NEW_VERSION}"

# Update VERSION file
echo "${NEW_VERSION}" > VERSION

# Update workspace version in root Cargo.toml (all crates inherit from here)
sed -i "s/version = \"${OLD_VERSION}\"/version = \"${NEW_VERSION}\"/g" Cargo.toml

echo "Done. Updated VERSION and workspace Cargo.toml (crates inherit via workspace = true)."
echo "Remember to update CHANGELOG.md"
