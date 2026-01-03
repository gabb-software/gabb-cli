# Hypothesis Driven Development

In hypothesis driven development we produce hypotheses about how the performance of the
gabb MCP server can be improved across the SWE-bench lite benchmark.

**All hypothesis tracking is done via GitHub Issues and PRs.** This provides:
- Native linking between issues and PRs
- Label-based status tracking with easy filtering
- Built-in history via closed issues
- Benchmark results documented in PR comments

## One-Time Setup: Labels

Create these labels in the repository (Settings → Labels → New label):

| Label | Color | Description |
|-------|-------|-------------|
| `hypothesis: untested` | `#d4c5f9` (light purple) | Hypothesis not yet tested |
| `hypothesis: testing` | `#fbca04` (yellow) | Actively being tested via PR |
| `hypothesis: proven` | `#0e8a16` (green) | Hypothesis confirmed by benchmarks |
| `hypothesis: disproven` | `#b60205` (red) | Hypothesis rejected by benchmarks |
| `hypothesis: investigating` | `#1d76db` (blue) | Needs further investigation |

## Hypothesis Lifecycle

```
┌─────────────────────────────────────────────────────────────┐
│  1. CREATE ISSUE                                            │
│     Use "Hypothesis" issue template                         │
│     Auto-labeled: hypothesis: untested                      │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│  2. CREATE PR                                               │
│     Branch: hypothesis/42-short-description                 │
│     Link to issue: "Relates to #42"                         │
│     Change label → hypothesis: testing                      │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│  3. RUN BENCHMARKS                                          │
│     Add results as PR comments                              │
│     20 runs for statistical significance                    │
└─────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────┐
│  4. CONCLUDE                                                │
│     Change label → hypothesis: proven OR disproven          │
│     Merge PR (if proven) or close (if disproven)            │
│     Close issue with final status                           │
└─────────────────────────────────────────────────────────────┘
```

## Issue Structure

Use the **Hypothesis** issue template, which captures:

| Field | Required | Description |
|-------|----------|-------------|
| Hypothesis | Yes | What change we believe will improve performance and why |
| Expected Improvement | Yes | Specific metrics we expect to change |
| Target SWE-bench Task | No | A task we expect to improve |
| Control SWE-bench Task | No | A task we expect to be unaffected |
| Source of Hypothesis | No | Where the hypothesis came from |
| Additional Context | No | Links, prior art, related issues |

### Example Issue Body

```markdown
## Hypothesis
We believe that softening the mandatory pre-read check guidance will improve
task success rate because the current strict wording causes Claude to call
gabb_structure even for trivial single-file tasks, adding latency without benefit.

## Expected Improvement
- Task success rate: No regression (maintain current level)
- Token usage: -5-10% on simple tasks
- Behavior: Fewer unnecessary gabb_structure calls on small files

## Target Task
django__django-12983 (involves reading multiple files)

## Control Task
astropy__astropy-6938 (single-file focused task)

## Source
Benchmark analysis showing gabb_structure called on files that were then
read in their entirety anyway.
```

## PR Structure

### Branch Naming
```
hypothesis/<issue-number>-<short-description>
```
Example: `hypothesis/42-soften-structure-guidance`

### PR Title
```
hypothesis(<scope>): <description>
```
Example: `hypothesis(mcp): soften gabb_structure guidance for simple tasks`

### PR Description Template

```markdown
## Hypothesis

Relates to #<issue-number>

<Copy or summarize the hypothesis from the issue>

## Implementation

<Describe what changes were made to test this hypothesis>

## Benchmark Plan

- [ ] Target task: <task-id> (20 runs)
- [ ] Control task: <task-id> (20 runs, if target improves)
- [ ] Wider task set (if control unaffected)

## Results

<Added as comments after each benchmark run>
```

### Benchmark Result Comments

Add a comment after each benchmark run:

```markdown
## Benchmark Run: <date>

**Task:** <task-id>
**Runs:** 20
**Branch:** hypothesis/42-... vs main

### Results

| Metric | Control (main) | Treatment (branch) | Δ |
|--------|---------------|-------------------|---|
| Success Rate | 60% (12/20) | 75% (15/20) | +15% |
| Avg Tokens | 45,000 | 42,000 | -7% |

### Statistical Analysis

<Chi-square test, confidence intervals, p-value>

### Conclusion

<Significant improvement / No significant difference / Regression detected>
```

## Querying Hypotheses

### Find all hypotheses by status

```bash
# Untested hypotheses
gh issue list --label "hypothesis: untested"

# Currently being tested
gh issue list --label "hypothesis: testing"

# Proven (history)
gh issue list --state closed --label "hypothesis: proven"

# Disproven (history)
gh issue list --state closed --label "hypothesis: disproven"

# All hypotheses ever
gh issue list --state all --search "label:hypothesis"
```

### Via GitHub Web UI

Use the Issues tab with label filters:
- `is:issue label:"hypothesis: proven" is:closed`
- `is:issue label:"hypothesis: untested" is:open`

## Process Summary

1. **Create Issue** using Hypothesis template → auto-labeled `hypothesis: untested`
2. **Create Branch** named `hypothesis/<issue>-<description>`
3. **Implement** changes to test the hypothesis
4. **Create PR** linking to issue with "Relates to #X"
5. **Update Label** to `hypothesis: testing`
6. **Run Benchmarks** on target task (20 runs), document in PR comments
7. **If improved:** Run on control task, then wider set
8. **Conclude:** Update label to `proven` or `disproven`
9. **Close:** Merge PR if proven, close if disproven; close issue

Disproven hypotheses are as valuable as proven ones—they prevent revisiting dead ends
and inform future hypotheses.
