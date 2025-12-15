#!/bin/bash
# Update homebrew formula with SHA256 hashes from a release
#
# Usage: ./update-formula.sh v0.1.0
#
# This script:
# 1. Downloads the SHA256SUMS.txt from the release
# 2. Updates the formula with the correct hashes
# 3. Outputs the updated formula

set -e

VERSION="${1:-}"

if [ -z "$VERSION" ]; then
    echo "Usage: $0 <version>"
    echo "Example: $0 v0.1.0"
    exit 1
fi

# Strip 'v' prefix if present for version number
VERSION_NUM="${VERSION#v}"

RELEASE_URL="https://github.com/gabb-software/gabb-cli/releases/download/${VERSION}"

echo "Fetching SHA256 sums for ${VERSION}..."

# Download SHA256SUMS.txt
SHA256_URL="${RELEASE_URL}/SHA256SUMS.txt"
SHA256_CONTENT=$(curl -sL "$SHA256_URL")

if [ -z "$SHA256_CONTENT" ]; then
    echo "Error: Could not fetch SHA256SUMS.txt from $SHA256_URL"
    exit 1
fi

echo "SHA256 sums:"
echo "$SHA256_CONTENT"
echo ""

# Extract hashes
AARCH64_SHA=$(echo "$SHA256_CONTENT" | grep "aarch64-apple-darwin" | awk '{print $1}')
X86_64_SHA=$(echo "$SHA256_CONTENT" | grep "x86_64-apple-darwin" | awk '{print $1}')
LINUX_SHA=$(echo "$SHA256_CONTENT" | grep "x86_64-unknown-linux-gnu" | awk '{print $1}')

if [ -z "$AARCH64_SHA" ] || [ -z "$X86_64_SHA" ] || [ -z "$LINUX_SHA" ]; then
    echo "Error: Could not extract all SHA256 hashes"
    exit 1
fi

echo "Extracted hashes:"
echo "  aarch64-apple-darwin: $AARCH64_SHA"
echo "  x86_64-apple-darwin:  $X86_64_SHA"
echo "  x86_64-linux-gnu:     $LINUX_SHA"
echo ""

# Generate updated formula
cat << EOF
# typed: false
# frozen_string_literal: true

class GabbCli < Formula
  desc "Fast local code indexing CLI for TypeScript and Rust projects"
  homepage "https://github.com/gabb-software/gabb-cli"
  version "${VERSION_NUM}"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/gabb-software/gabb-cli/releases/download/v#{version}/gabb-cli-aarch64-apple-darwin.tar.gz"
      sha256 "${AARCH64_SHA}"
    else
      url "https://github.com/gabb-software/gabb-cli/releases/download/v#{version}/gabb-cli-x86_64-apple-darwin.tar.gz"
      sha256 "${X86_64_SHA}"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/gabb-software/gabb-cli/releases/download/v#{version}/gabb-cli-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "${LINUX_SHA}"
    end
  end

  def install
    bin.install "gabb-cli"
  end

  test do
    assert_match "gabb-cli", shell_output("#{bin}/gabb-cli --version")
  end
end
EOF
