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

# Update all Cargo.toml files
find . -name "Cargo.toml" -not -path "*/target/*" -exec \
    sed -i "s/version = \"${OLD_VERSION}\"/version = \"${NEW_VERSION}\"/g" {} +

echo "Done. Updated VERSION and all Cargo.toml files."
echo "Remember to update CHANGELOG.md"
