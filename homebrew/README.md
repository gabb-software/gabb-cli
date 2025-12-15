# Homebrew Distribution

This directory contains the Homebrew formula and tooling for distributing gabb-cli via Homebrew.

## Setup (One-time)

1. Create a new GitHub repository: `gabb-software/homebrew-tap`

2. In that repository, create a `Formula` directory and copy `gabb-cli.rb` into it:
   ```
   homebrew-tap/
   └── Formula/
       └── gabb-cli.rb
   ```

3. Users can then install with:
   ```bash
   brew tap gabb-software/tap
   brew install gabb-cli
   ```

## Release Process

### 1. Update version in Cargo.toml

```bash
# Edit Cargo.toml and update version
vim Cargo.toml
```

### 2. Create and push a version tag

```bash
git add -A
git commit -m "chore: bump version to 0.2.0"
git tag v0.2.0
git push origin main --tags
```

### 3. Wait for GitHub Actions to build

The release workflow will:
- Build binaries for macOS (x86_64, aarch64) and Linux (x86_64)
- Create a universal macOS binary
- Create a GitHub release with all artifacts
- Generate SHA256SUMS.txt

### 4. Update the Homebrew formula

After the release is published, update the formula in the homebrew-tap repo:

```bash
# Generate updated formula with correct SHA256 hashes
./update-formula.sh v0.2.0 > /path/to/homebrew-tap/Formula/gabb-cli.rb

# Or manually:
# 1. Download SHA256SUMS.txt from the release
# 2. Update version and sha256 values in the formula
# 3. Commit and push to homebrew-tap
```

### 5. Verify installation

```bash
brew update
brew upgrade gabb-cli
# or for new installs:
brew tap gabb-software/tap
brew install gabb-cli
```

## Automated Updates (Optional)

To fully automate formula updates, you can:

1. Create a Personal Access Token (PAT) with repo access to homebrew-tap
2. Add it as a secret `HOMEBREW_TAP_TOKEN` in gabb-cli repo
3. Update the release workflow to automatically push formula updates

See the commented section in `.github/workflows/release.yml` for details.

## Testing the Formula Locally

```bash
# Test the formula before publishing
brew install --build-from-source ./gabb-cli.rb

# Or test from the tap
brew tap gabb-software/tap
brew install --verbose gabb-cli
```

## Formula Types

### Pre-built binaries (current)

The formula downloads pre-built binaries from GitHub releases. This is:
- Faster to install (no compilation)
- More reliable (no build dependencies)
- Larger download size

### Build from source (alternative)

If you prefer users to build from source:

```ruby
class GabbCli < Formula
  desc "Fast local code indexing CLI"
  homepage "https://github.com/gabb-software/gabb-cli"
  url "https://github.com/gabb-software/gabb-cli/archive/v0.1.0.tar.gz"
  sha256 "SOURCE_TARBALL_SHA256"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match "gabb-cli", shell_output("#{bin}/gabb-cli --version")
  end
end
```
