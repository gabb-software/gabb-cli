# Release

Create a new release by analyzing changes and prompting for version bump type.

## Step 1: Get Current Version and Last Release Commit

Read the current version from `version.toml`:

```bash
cat version.toml
```

Find the last release commit (identified by `chore: release vX.Y.Z` message):

```bash
git log --oneline --grep="^chore: release v" -1
```

## Step 2: Analyze Changes Since Last Release

Get all commits since the last release commit (excluding the release commit itself):

```bash
LAST_RELEASE=$(git log --format="%H" --grep="^chore: release v" -1)
git log ${LAST_RELEASE}..HEAD --oneline --no-merges
```

Also get the full commit messages for better analysis:

```bash
LAST_RELEASE=$(git log --format="%H" --grep="^chore: release v" -1)
git log ${LAST_RELEASE}..HEAD --pretty=format:"- %s%n  %b" --no-merges
```

If no release commit exists (first release), use the last 50 commits:

```bash
git log HEAD~50..HEAD --oneline --no-merges
```

## Step 3: Categorize Changes

Analyze the commits and categorize them:

### Breaking Changes (→ MAJOR)
Look for commits with:
- `BREAKING CHANGE:` in the body
- Commits with breaking change indicator suffix on type (like feat!: or fix!:)
- Commits mentioning "breaking", "incompatible", "remove API", "rename public"

### New Features (→ MINOR)
Look for commits with:
- `feat:` prefix
- Adding new commands, tools, or capabilities
- New language support
- New CLI options or flags

### Bug Fixes and Improvements (→ PATCH)
Look for commits with:
- `fix:` prefix
- `perf:` prefix
- `refactor:` (internal only)
- `docs:`, `chore:`, `ci:`, `test:` (no version bump needed, but include in patch if releasing)

## Step 4: Generate Recommendation

Based on the analysis, determine:
1. **Recommended version type** (major/minor/patch)
2. **Rationale** - Brief explanation of why this level is recommended
3. **Changelog summary** - Key changes grouped by type

Format the recommendation clearly:

```
Current version: X.Y.Z
Last release: vX.Y.Z (from last "chore: release" commit, or "No previous releases")

📊 Changes since last release:
- N breaking changes
- N new features
- N bug fixes
- N other changes

🎯 Recommended: [MAJOR|MINOR|PATCH] release → vX.Y.Z

Rationale: [Brief explanation]

Key changes:
- [Grouped summary of important changes]
```

## Step 5: Prompt User for Confirmation

Use the AskUserQuestion tool with these options:

**Question: What type of release should this be?**

Options (order by recommendation - put recommended first with "(Recommended)" suffix):
1. **Major** - Breaking changes, major milestones (X+1.0.0)
2. **Minor** - New features, significant improvements (X.Y+1.0)
3. **Patch** - Bug fixes, small improvements (X.Y.Z+1)
4. **Cancel** - Don't create a release

Include the rationale and key changes in the descriptions.

## Step 6: Update Version and Create Release

If user confirms (not Cancel):

### 6a. Calculate New Version
Based on current version (major.minor.patch) and selected type:
- **Major**: (major+1).0.0
- **Minor**: major.(minor+1).0
- **Patch**: major.minor.(patch+1)

### 6b. Update version.toml

Edit `version.toml` to set the new version numbers. Keep the comments intact.

### 6c. Commit the Change

```bash
git add version.toml
git commit -m "chore: release vX.Y.Z"
```

Where X.Y.Z is the new version.

### 6d. Push to Trigger Release

Ask user if they want to push now:

**Question: Push to trigger release workflow?**
- **Push now** - Push to main to trigger release (Recommended)
- **Manual push** - I'll push manually later

If push now:
```bash
git push origin main
```

## Step 7: Confirm Success

Display confirmation:

```
✅ Release vX.Y.Z prepared!

Version file updated and committed.
[If pushed] Release workflow triggered - check GitHub Actions.
[If not pushed] Run 'git push origin main' to trigger the release.
```

## Version Guidelines Reference

From CLAUDE.md:
- **major**: Breaking changes or major milestones
- **minor**: New features or significant improvements
- **patch**: Bug fixes and small improvements
