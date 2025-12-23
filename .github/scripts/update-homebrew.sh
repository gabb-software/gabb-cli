#!/usr/bin/env bash
# Update Homebrew formula in gabb-software/homebrew-tap
#
# Required environment variables:
#   VERSION         - Version string (e.g., 1.2.3)
#   SHA256_AARCH64  - SHA256 hash for aarch64-apple-darwin
#   SHA256_X86_64   - SHA256 hash for x86_64-apple-darwin
#   SHA256_LINUX    - SHA256 hash for x86_64-unknown-linux-musl
#   GITHUB_TOKEN    - Token with push access to homebrew-tap repo

set -euo pipefail

REPO_DIR="homebrew-tap"
TEMPLATE_PATH=".github/templates/gabb.rb.template"
FORMULA_PATH="${REPO_DIR}/Formula/gabb.rb"

echo "Updating Homebrew formula to version ${VERSION}"

# Clone the tap repo
git clone "https://x-access-token:${GITHUB_TOKEN}@github.com/gabb-software/homebrew-tap.git" "${REPO_DIR}"

# Generate formula from template
sed -e "s/{{VERSION}}/${VERSION}/g" \
    -e "s/{{SHA256_AARCH64}}/${SHA256_AARCH64}/g" \
    -e "s/{{SHA256_X86_64}}/${SHA256_X86_64}/g" \
    -e "s/{{SHA256_LINUX}}/${SHA256_LINUX}/g" \
    "${TEMPLATE_PATH}" > "${FORMULA_PATH}"

echo "Generated formula:"
cat "${FORMULA_PATH}"

# Commit and push
cd "${REPO_DIR}"
git config user.name "github-actions[bot]"
git config user.email "github-actions[bot]@users.noreply.github.com"
git add Formula/gabb.rb
git commit -m "Update gabb to ${VERSION}"
git push

echo "Homebrew formula updated successfully"
