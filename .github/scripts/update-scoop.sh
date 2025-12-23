#!/usr/bin/env bash
# Update Scoop manifest in gabb-software/scoop-bucket
#
# Required environment variables:
#   VERSION         - Version string (e.g., 1.2.3)
#   SHA256_WINDOWS  - SHA256 hash for x86_64-pc-windows-msvc
#   GITHUB_TOKEN    - Token with push access to scoop-bucket repo

set -euo pipefail

REPO_DIR="scoop-bucket"
TEMPLATE_PATH=".github/templates/gabb.json.template"
MANIFEST_PATH="${REPO_DIR}/gabb.json"

echo "Updating Scoop manifest to version ${VERSION}"

# Clone the bucket repo
git clone "https://x-access-token:${GITHUB_TOKEN}@github.com/gabb-software/scoop-bucket.git" "${REPO_DIR}"

# Generate manifest from template
sed -e "s/{{VERSION}}/${VERSION}/g" \
    -e "s/{{SHA256_WINDOWS}}/${SHA256_WINDOWS}/g" \
    "${TEMPLATE_PATH}" > "${MANIFEST_PATH}"

echo "Generated manifest:"
cat "${MANIFEST_PATH}"

# Commit and push
cd "${REPO_DIR}"
git config user.name "github-actions[bot]"
git config user.email "github-actions[bot]@users.noreply.github.com"
git add gabb.json
git commit -m "Update gabb to ${VERSION}"
git push

echo "Scoop manifest updated successfully"
