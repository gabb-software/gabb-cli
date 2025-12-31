# Release Process

This document describes how to release gabb-cli for different platforms.

## Overview

gabb-cli uses two GitHub Actions workflows:

- **CI workflow** (`ci.yml`): Runs tests and linting on every push to main and all PRs
- **Release workflow** (`release.yml`): Triggers when `version.toml` is modified, runs full CI, then builds and publishes release binaries

The release workflow produces:

- **macOS**: x86_64, aarch64, and universal binaries
- **Linux**: x86_64 binary (musl, statically linked)
- **Windows**: x86_64 binary

## Prerequisites

- Push access to the repository
- For Homebrew: access to the `gabb-software/homebrew-tap` repository

## Creating a Release

### 1. Update Version

Edit `version.toml` and update the version numbers:

```toml
major = 0
minor = 10
patch = 1  # Increment this for bug fixes
```

Version guidelines:
- **major**: Breaking changes or major milestones
- **minor**: New features or significant improvements
- **patch**: Bug fixes and small improvements

### 2. Update Changelog (Optional)

If you maintain a CHANGELOG.md, update it with the new version's changes.

### 3. Commit and Push

```bash
git add version.toml
git commit -m "chore: release v0.10.1"
git push origin main
```

The release workflow automatically triggers when `version.toml` changes.

### 4. Wait for GitHub Actions

The release workflow will:
1. Run full test suite on Ubuntu and macOS
2. Run linting (rustfmt, clippy)
3. Build binaries for all platforms
4. Create a universal macOS binary
5. Generate SHA256 checksums
6. Create git tag and GitHub release
7. Update Homebrew and Scoop package managers

Monitor progress at: `https://github.com/gabb-software/gabb-cli/actions`

### 5. Verify Release

Once complete, verify the release at:
`https://github.com/gabb-software/gabb-cli/releases/tag/v0.10.1`

Expected artifacts:
- `gabb-x86_64-apple-darwin.tar.gz`
- `gabb-aarch64-apple-darwin.tar.gz`
- `gabb-universal-apple-darwin.tar.gz`
- `gabb-x86_64-unknown-linux-musl.tar.gz`
- `gabb-x86_64-pc-windows-msvc.zip`
- `SHA256SUMS.txt`

---

## macOS (Homebrew)

### First-Time Setup

1. Create the tap repository on GitHub: `gabb-software/homebrew-tap`

2. Clone it locally:
   ```bash
   git clone https://github.com/gabb-software/homebrew-tap.git
   cd homebrew-tap
   mkdir Formula
   ```

3. Copy the initial formula:
   ```bash
   cp /path/to/gabb-cli/homebrew/gabb-cli.rb Formula/
   ```

### Updating the Formula

After each release:

1. Generate the updated formula with correct SHA256 hashes:
   ```bash
   cd /path/to/gabb-cli
   ./homebrew/update-formula.sh v0.2.0
   ```

2. Copy the output to the tap repository:
   ```bash
   ./homebrew/update-formula.sh v0.2.0 > /path/to/homebrew-tap/Formula/gabb-cli.rb
   ```

3. Commit and push:
   ```bash
   cd /path/to/homebrew-tap
   git add Formula/gabb-cli.rb
   git commit -m "Update gabb-cli to 0.2.0"
   git push origin main
   ```

### User Installation

Users install with:
```bash
brew tap gabb-software/tap
brew install gabb-cli

# Upgrade existing installation
brew update && brew upgrade gabb-cli
```

### Testing the Formula

```bash
# Test installation from tap
brew install --verbose gabb-software/tap/gabb-cli

# Test local formula file
brew install --build-from-source ./Formula/gabb-cli.rb
```

---

## Linux

### Package Formats

Currently, Linux releases are distributed as tarballs. Future options include:

| Format | Distribution | Status |
|--------|--------------|--------|
| `.tar.gz` | Generic | ✅ Available |
| `.deb` | Debian/Ubuntu | 🔜 Planned |
| `.rpm` | Fedora/RHEL | 🔜 Planned |
| AUR | Arch Linux | 🔜 Planned |
| Nix | NixOS | 🔜 Planned |

### Manual Installation (Current)

```bash
# Download latest release
VERSION="0.10.1"
curl -LO "https://github.com/gabb-software/gabb-cli/releases/download/v${VERSION}/gabb-x86_64-unknown-linux-musl.tar.gz"

# Verify checksum
curl -LO "https://github.com/gabb-software/gabb-cli/releases/download/v${VERSION}/SHA256SUMS.txt"
sha256sum -c SHA256SUMS.txt --ignore-missing

# Extract and install
tar -xzf gabb-x86_64-unknown-linux-musl.tar.gz
sudo mv gabb /usr/local/bin/
```

### Installation Script (Recommended)

Create an install script for users:

```bash
#!/bin/bash
set -e

VERSION="${1:-latest}"
INSTALL_DIR="${2:-/usr/local/bin}"

if [ "$VERSION" = "latest" ]; then
    VERSION=$(curl -s https://api.github.com/repos/gabb-software/gabb-cli/releases/latest | grep tag_name | cut -d'"' -f4)
fi

echo "Installing gabb ${VERSION}..."

ARCH=$(uname -m)
case "$ARCH" in
    x86_64) TARGET="x86_64-unknown-linux-musl" ;;
    aarch64) TARGET="aarch64-unknown-linux-musl" ;;
    *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

curl -LO "https://github.com/gabb-software/gabb-cli/releases/download/${VERSION}/gabb-${TARGET}.tar.gz"
tar -xzf "gabb-${TARGET}.tar.gz"
sudo mv gabb "$INSTALL_DIR/"
rm "gabb-${TARGET}.tar.gz"

echo "Installed gabb to ${INSTALL_DIR}/gabb"
gabb --version
```

### Adding Debian/Ubuntu Packages (Future)

To add `.deb` package support:

1. Add `cargo-deb` to the release workflow:
   ```yaml
   - name: Build deb package
     run: |
       cargo install cargo-deb
       cargo deb
   ```

2. Add `[package.metadata.deb]` section to `Cargo.toml`:
   ```toml
   [package.metadata.deb]
   maintainer = "Your Name <your@email.com>"
   copyright = "2025, Gabb Software"
   depends = "$auto"
   section = "utility"
   priority = "optional"
   assets = [
       ["target/release/gabb-cli", "usr/bin/", "755"],
   ]
   ```

### Adding RPM Packages (Future)

To add `.rpm` package support:

1. Add `cargo-generate-rpm` to the release workflow
2. Add `[package.metadata.rpm]` section to `Cargo.toml`

### Adding to AUR (Future)

Create a PKGBUILD file for Arch Linux:

```bash
# Maintainer: Your Name <your@email.com>
pkgname=gabb-cli
pkgver=0.2.0
pkgrel=1
pkgdesc="Fast local code indexing CLI"
arch=('x86_64')
url="https://github.com/gabb-software/gabb-cli"
license=('MIT')
depends=('gcc-libs')
makedepends=('rust' 'cargo')
source=("$pkgname-$pkgver.tar.gz::https://github.com/gabb-software/gabb-cli/archive/v$pkgver.tar.gz")
sha256sums=('SKIP')

build() {
    cd "$pkgname-$pkgver"
    cargo build --release --locked
}

package() {
    cd "$pkgname-$pkgver"
    install -Dm755 "target/release/gabb-cli" "$pkgdir/usr/bin/gabb-cli"
    install -Dm644 "LICENSE" "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
}
```

---

## Windows

### Current Status

Windows builds are automated via GitHub Actions. Each release includes a Windows binary.

### Installation via Scoop

```powershell
# Add the bucket (first time only)
scoop bucket add gabb https://github.com/gabb-software/scoop-bucket

# Install
scoop install gabb
```

### Manual Installation

Download `gabb-x86_64-pc-windows-msvc.zip` from the latest release and extract to a directory in your PATH.

### Building from Source

```powershell
# Install Rust from https://rustup.rs
# Then:
git clone https://github.com/gabb-software/gabb-cli.git
cd gabb-cli
cargo build --release

# Binary will be at: target\release\gabb.exe
```

### Chocolatey Package (Future)

To distribute via Chocolatey:

1. Create a `choco/` directory with:
   - `gabb-cli.nuspec` - Package metadata
   - `tools/chocolateyinstall.ps1` - Install script

2. Example nuspec:
   ```xml
   <?xml version="1.0" encoding="utf-8"?>
   <package xmlns="http://schemas.microsoft.com/packaging/2015/06/nuspec.xsd">
     <metadata>
       <id>gabb-cli</id>
       <version>0.2.0</version>
       <title>gabb-cli</title>
       <authors>Gabb Software</authors>
       <projectUrl>https://github.com/gabb-software/gabb-cli</projectUrl>
       <licenseUrl>https://github.com/gabb-software/gabb-cli/blob/main/LICENSE</licenseUrl>
       <description>Fast local code indexing CLI</description>
       <tags>cli code-indexing development</tags>
     </metadata>
     <files>
       <file src="tools\**" target="tools" />
     </files>
   </package>
   ```

### WinGet Package (Future)

To distribute via Windows Package Manager:

1. Fork `microsoft/winget-pkgs`
2. Add manifest at `manifests/g/GabbSoftware/gabb-cli/0.2.0/`
3. Submit PR to winget-pkgs

---

## Cargo (crates.io)

### Publishing to crates.io

```bash
# Login (first time only)
cargo login

# Publish
cargo publish
```

### Prerequisites for crates.io

1. Ensure all dependencies are on crates.io
2. Add required metadata to `Cargo.toml`:
   ```toml
   [package]
   license = "MIT"
   description = "..."
   repository = "..."
   ```

### Installation via Cargo

Users can install with:
```bash
cargo install gabb-cli
```

---

## Release Checklist

```markdown
## Release v0.X.Y

- [ ] Update version in `version.toml` (major, minor, and/or patch)
- [ ] Update CHANGELOG.md (if maintained)
- [ ] Run full test suite locally: `cargo test`
- [ ] Run clippy locally: `cargo clippy --all-targets --all-features`
- [ ] Commit and push: `git commit -m "chore: release v0.X.Y" && git push`
- [ ] Wait for GitHub Actions release workflow to complete
- [ ] Verify release artifacts on GitHub releases page
- [ ] Test installation: `brew upgrade gabb` (macOS) or `scoop update gabb` (Windows)
- [ ] Announce release (if applicable)
```

---

## Troubleshooting

### Release workflow failed

1. Check the Actions tab for error details
2. Common issues:
   - Rust compilation errors
   - Missing targets (run `rustup target add <target>`)
   - Rate limiting on artifact uploads

### Homebrew formula errors

```bash
# Debug formula issues
brew install --verbose --debug gabb-software/tap/gabb-cli

# Check formula syntax
brew audit --strict Formula/gabb-cli.rb
```

### Binary doesn't run on older macOS

The default build targets the runner's macOS version. To support older versions:

```yaml
env:
  MACOSX_DEPLOYMENT_TARGET: "10.14"
```

### Linux binary compatibility

The Linux build uses musl for static linking, which should work on any Linux distribution. If you encounter issues, you can build from source with your system's glibc:

```bash
git clone https://github.com/gabb-software/gabb-cli.git
cd gabb-cli
cargo build --release
```
