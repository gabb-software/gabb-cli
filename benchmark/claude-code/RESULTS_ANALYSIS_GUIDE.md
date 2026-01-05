# Benchmark Results Analysis Guide

This guide explains how to analyze Claude Code benchmark results, including reading transcripts, interpreting metrics, performing statistical tests, and producing summary reports.

## Quick Reference

| What You Need | Where To Find It |
|--------------|------------------|
| Benchmark results | `benchmark/claude-code/results/*.json` |
| Claude transcripts | `~/.claude/projects/{workspace-path}/*.jsonl` |
| Telemetry tools | `benchmark/telemetry/` |
| Current summary | `benchmark/claude-code/README.md` |
| Past analyses | `benchmark/claude-code/analysis/*.md` |

---

## 1. Understanding Results Files

### Results JSON Structure

Each benchmark run produces a JSON file in `benchmark/claude-code/results/`:

```
results_{task_id}_n{run_count}_{timestamp}.json
```

Example: `results_astropy__astropy-12907_n10_20251229_104532.json`

**Key fields:**

```json
{
  "task_id": "astropy__astropy-12907",
  "timestamp": "20251229_104532",
  "run_count": 10,
  "conditions": {
    "control": {
      "runs": [...],      // Individual run data
      "aggregate": {...}  // Statistical summary
    },
    "gabb": {
      "runs": [...],
      "aggregate": {...}
    }
  }
}
```

### Suite Results Structure

Suite results aggregate multiple tasks into a single file:

```
suite_results_{branch}_{commit}_{n}{run_count}_{timestamp}.json
```

Example: `suite_results_fix-102-tune-gabb-st_6311560_n10_20260104_094005.json`

**Suite structure differs from individual task results:**

```json
{
  "timestamp": "20260104_094005",
  "task_count": 10,
  "run_count": 10,
  "branch_info": {
    "branch": "fix-102-tune-gabb-structure-guidance",
    "commit_sha": "6311560"
  },
  "summary": {
    "control": {
      "wall_time_seconds": {"mean": 48.57, "std": 30.07, ...},
      "tokens_total": {"mean": 61320.87, "std": 23320.8, ...},
      "success_rate": 0.95,
      "run_count": 100,  // task_count * runs_per_task
      "tool_calls": {...}
    },
    "gabb": {...}
  },
  "tasks": [
    {
      "task_id": "django__django-11179",
      "control": {
        "runs": [...],  // Note: no "aggregate" key at task level
        "success_rate": 1.0
      },
      "gabb": {
        "runs": [...],
        "success_rate": 1.0
      }
    }
  ]
}
```

**Key differences from individual task results:**

| Aspect | Individual Task | Suite |
|--------|-----------------|-------|
| Top-level stats | `conditions.{name}.aggregate` | `summary.{name}` |
| Per-task data | N/A | `tasks[].{condition}` |
| Run count | Runs for one task | Total runs across all tasks |
| Metadata | `task_id` | `task_count`, `branch_info` |

**Branch metadata (`branch_info`):**

Suite results include git branch information for tracking which code version was tested:
- `branch`: Full branch name
- `commit_sha`: Short commit SHA

### Per-Run Metrics

Each run in the `runs` array contains:

| Field | Description |
|-------|-------------|
| `wall_time_seconds` | Total execution time |
| `tokens_input` | Input tokens consumed |
| `tokens_output` | Output tokens generated |
| `tokens_total` | Sum of input + output |
| `tokens_cache_read` | Tokens read from cache (if available) |
| `tokens_cache_create` | Tokens written to cache (if available) |
| `tool_calls` | Map of tool name → call count |
| `tool_calls_total` | Total number of tool invocations |
| `turns` | Conversation turns |
| `success` | Whether task was completed correctly |
| `final_answer` | Claude's final response |
| `cost_usd` | Estimated cost in USD (if available) |

### Aggregate Statistics

The `aggregate` section provides:

```json
{
  "wall_time_seconds": {"mean": 45.2, "std": 12.3, "min": 28.1, "max": 72.4},
  "tokens_total": {"mean": 150000, "std": 45000, ...},
  "success_rate": 0.9,
  "success_count": 9,
  "run_count": 10,
  "tool_calls": {
    "Grep": {"mean": 4.2, "std": 1.8, ...},
    "Read": {"mean": 5.1, "std": 2.3, ...}
  }
}
```

### Cache Token Economics

Understanding token costs requires knowing how caching affects pricing:

| Token Type | Cost Multiplier | Description |
|------------|-----------------|-------------|
| Cache Read | 10% (0.1x) | Tokens retrieved from prompt cache |
| Cache Create | 125% (1.25x) | Tokens written to cache (first use) |
| Input | 100% (1x) | Regular input tokens |
| Output | 500% (5x) | Generated output tokens |

**Why this matters:** A run with higher total tokens may cost the same or less if most tokens are cache reads. Always report both raw tokens AND estimated cost.

**Calculating effective cost:**
```python
def calculate_cost(run, input_price_per_mtok=3.00, output_price_per_mtok=15.00):
    """Calculate cost with Anthropic's prompt caching pricing."""
    cache_read = run.get('tokens_cache_read', 0)
    cache_create = run.get('tokens_cache_create', 0)
    input_tokens = run.get('tokens_input', 0)
    output_tokens = run.get('tokens_output', 0)

    # Cache read is 10% of input price, cache create is 125%
    input_cost = (
        (cache_read * 0.10 + cache_create * 1.25 + input_tokens)
        * input_price_per_mtok / 1_000_000
    )
    output_cost = output_tokens * output_price_per_mtok / 1_000_000

    return input_cost + output_cost
```

---

## 2. Reading Claude Transcripts

### Transcript Location

Claude Code stores conversation transcripts in:

```
~/.claude/projects/{sanitized-workspace-path}/{session-id}.jsonl
```

For benchmark runs, workspace paths look like:
```
-Users-dmb--cache-gabb-benchmark-repos-workspaces-astropy--astropy--{hash}
```

### Finding Benchmark Transcripts

```bash
# List recent benchmark workspace directories
ls -lt ~/.claude/projects/ | grep gabb-benchmark | head -10

# List transcripts in a workspace (most recent first)
ls -lt ~/.claude/projects/-Users-dmb--cache-gabb-benchmark-repos-workspaces-astropy--astropy--*/

# View a specific transcript
cat ~/.claude/projects/{path}/{session-id}.jsonl | python3 -m json.tool
```

### Matching Transcripts to Benchmark Runs

Benchmark results don't directly link to transcript files. To find transcripts for a specific run:

1. Use the benchmark timestamp to narrow down the time window
2. List transcripts modified around that time: `ls -lt ~/.claude/projects/{workspace}/*.jsonl`
3. Match by examining the first user message (contains the task prompt)

### Transcript Format (JSONL)

Each line is a JSON object. Key message types:

```json
// User message
{"type": "user", "message": {"role": "user", "content": "..."}}

// Assistant message with tool use
{"type": "assistant", "message": {
  "role": "assistant",
  "content": [
    {"type": "text", "text": "..."},
    {"type": "tool_use", "name": "Grep", "input": {...}}
  ]
}}

// Tool result
{"type": "tool_result", "content": "..."}
```

### Extracting Tool Calls from Transcripts

```bash
# Count tool calls by type
cat transcript.jsonl | python3 -c "
import sys, json
tools = {}
for line in sys.stdin:
    msg = json.loads(line)
    if 'message' in msg and 'content' in msg['message']:
        for block in msg['message'].get('content', []):
            if isinstance(block, dict) and block.get('type') == 'tool_use':
                name = block.get('name', 'unknown')
                tools[name] = tools.get(name, 0) + 1
for name, count in sorted(tools.items(), key=lambda x: -x[1]):
    print(f'{name}: {count}')
"
```

---

## 3. Using the Telemetry Tools

The `benchmark/telemetry/` package provides analysis utilities.

### Installation

```bash
cd benchmark/telemetry
pip install -e .
```

### Analyzing Transcripts

```bash
# Analyze a single transcript
gabb-benchmark analyze ~/.claude/projects/{path}/{session}.jsonl

# JSON output for further processing
gabb-benchmark analyze transcript.jsonl --format json > report.json

# Analyze multiple transcripts
gabb-benchmark analyze *.jsonl --summary
```

### Output Includes

- **Token summary**: Total turns, tool calls, input/output/file content tokens
- **Tool distribution**: Call counts and token consumption per tool
- **Bash breakdown**: grep, find, cat, etc. usage within Bash calls

---

## 4. Key Metrics to Compare

### Primary Metrics

| Metric | What It Shows | Target |
|--------|---------------|--------|
| **Success Rate** | Task completion accuracy | Higher is better |
| **Time (seconds)** | Wall-clock execution time | Lower is better |
| **Total Tokens** | Raw token consumption | Lower is better |
| **Cost (USD)** | Actual cost accounting for caching | Lower is better |
| **Tool Calls** | Navigation efficiency | Lower is better |

### Tool Usage Patterns

Compare how Claude navigates code:

| Tool | Control Pattern | Gabb Pattern |
|------|-----------------|--------------|
| Grep | High usage for searching | Low (replaced by gabb_symbols) |
| Read | High (full file reads) | Low (targeted reads after gabb_structure) |
| Glob | Moderate (finding files) | Low |
| gabb_symbols | N/A | Primary search tool |
| gabb_structure | N/A | Pre-read file overview |

### What to Look For

1. **Time Improvement**: Faster completion due to:
   - More direct navigation path
   - Less trial-and-error searching

2. **Tool Call Reduction**: Gabb should make fewer calls:
   - Direct symbol lookup vs. grep→read→grep cycles
   - Structure preview before reading

3. **Token Changes**: Results vary by task:
   - *Expected*: Fewer tokens from targeted reads and semantic search
   - *Observed (2026-01-01)*: +24% tokens, but $0 cost increase due to caching
   - Always compare **cost**, not just token counts

4. **Consistent Success**: Both conditions should succeed equally

---

## 5. Statistical Significance Testing

**Why this section matters:** Raw percentage differences (e.g., "50% faster") are meaningless without knowing if they could have occurred by chance. This section teaches you to distinguish real effects from noise.

### Two-Sample t-Test

Use this to determine if the difference between Control and Gabb is statistically significant.

```python
import math

def two_sample_t_test(mean1, std1, n1, mean2, std2, n2):
    """
    Perform Welch's t-test (unequal variances).
    Returns t-statistic and approximate degrees of freedom.
    """
    # Standard errors
    se1 = std1 / math.sqrt(n1)
    se2 = std2 / math.sqrt(n2)

    # Standard error of difference
    se_diff = math.sqrt(se1**2 + se2**2)

    # t-statistic
    t_stat = (mean1 - mean2) / se_diff

    # Welch-Satterthwaite degrees of freedom
    df = (se1**2 + se2**2)**2 / (
        se1**4 / (n1 - 1) + se2**4 / (n2 - 1)
    )

    return t_stat, df, se_diff

# Example from 2026-01-01 analysis
control_time = (60.2, 16.4, 50)  # mean, std, n
gabb_time = (29.9, 19.0, 50)

t_stat, df, se_diff = two_sample_t_test(*control_time, *gabb_time)
print(f"t-statistic: {t_stat:.2f}")
print(f"Degrees of freedom: {df:.1f}")
print(f"Standard error of difference: {se_diff:.2f}")
```

### Interpreting Results

| |t| value | p-value (approx) | Interpretation |
|------------|------------------|------------------|
| < 1.0 | > 0.30 | Not significant |
| 1.0 - 2.0 | 0.05 - 0.30 | Weak evidence |
| 2.0 - 3.0 | 0.01 - 0.05 | Significant (*) |
| 3.0 - 4.0 | 0.001 - 0.01 | Highly significant (**) |
| > 4.0 | < 0.001 | Very highly significant (***) |

**Rule of thumb:** For n=50, |t| > 2.0 indicates statistical significance at p < 0.05.

### Confidence Intervals

A 95% confidence interval tells you the range where the true difference likely lies:

```python
def confidence_interval_95(mean_diff, se_diff, df):
    """Calculate 95% CI. Uses t-critical ≈ 2.0 for large df."""
    # For df > 30, t_critical ≈ 2.0
    t_critical = 2.0 if df > 30 else 2.1  # approximate

    lower = mean_diff - t_critical * se_diff
    upper = mean_diff + t_critical * se_diff

    return lower, upper

# Example: time reduction
mean_diff = 60.2 - 29.9  # = 30.3s
lower, upper = confidence_interval_95(mean_diff, se_diff=3.55, df=98)
print(f"95% CI for time reduction: [{lower:.1f}s, {upper:.1f}s]")
```

### Effect Size (Cohen's d)

Statistical significance tells you if an effect is real; effect size tells you if it matters.

```python
def cohens_d(mean1, std1, mean2, std2):
    """Calculate Cohen's d effect size."""
    pooled_std = ((std1**2 + std2**2) / 2) ** 0.5
    return (mean1 - mean2) / pooled_std

# Example
d = cohens_d(60.2, 16.4, 29.9, 19.0)  # ≈ 1.7
```

| Cohen's d | Interpretation |
|-----------|----------------|
| 0.2 | Small effect |
| 0.5 | Medium effect |
| 0.8 | Large effect |
| > 1.0 | Very large effect |

### Sample Size Guidelines

| Runs | Suitable For |
|------|--------------|
| 10 | Quick sanity check, large effects only (d > 1.0) |
| 20 | Publication-quality for main effects (d > 0.5) |
| 50 | Subgroup analysis, medium effect sizes |
| 100+ | Small effect sizes (d < 0.5), multiple comparisons |

**When to run more:**
- High variance (std > 50% of mean)
- Effect size is small (d < 0.5)
- Need subgroup comparisons

---

## 6. Subgroup Analysis & Selection Bias

### The Selection Bias Trap

**WARNING:** Naive comparisons can be misleading. Consider this scenario:

> "Runs that used gabb_structure took LONGER than runs that didn't use it!"

This could be **selection bias**, not a tool problem:

1. Easy runs (solved without reading files) never call gabb_structure
2. Hard runs (require file exploration) do call gabb_structure
3. Comparing all runs conflates task difficulty with tool effectiveness

### Proper Subgroup Analysis

To evaluate gabb_structure specifically, compare **only runs that read files**.

**Note:** This analysis uses per-run data from `conditions.{condition}.runs[]`, not the aggregate statistics. Each run object contains its own `tool_calls` map.

```python
def analyze_by_subgroup(runs):
    """Group runs by behavior pattern."""
    groups = {
        '0_reads': [],
        '1_read': [],
        '2+_reads': []
    }

    for run in runs:
        read_count = run['tool_calls'].get('Read', 0)
        if read_count == 0:
            groups['0_reads'].append(run)
        elif read_count == 1:
            groups['1_read'].append(run)
        else:
            groups['2+_reads'].append(run)

    return groups

# Then compare gabb_structure usage WITHIN the file-reading groups
```

### What to Report

For each subgroup, report:

| Subgroup | N | Avg Time | Avg Tokens | gabb_structure Usage |
|----------|---|----------|------------|---------------------|
| 0 reads | 17 | 17.3s | 69,035 | 0% (expected) |
| 1 read | 23 | 22.5s | 91,000 | 78% |
| 2+ reads | 10 | 52.6s | 60,000 | 60% |

### Behavioral Pattern Analysis

Look for distinct patterns in how Claude approaches tasks:

```python
def identify_patterns(gabb_runs):
    """Identify behavioral patterns in Gabb condition."""
    patterns = []

    for run in gabb_runs:
        reads = run['tool_calls'].get('Read', 0)
        structure = run['tool_calls'].get('mcp__gabb__gabb_structure', 0)

        if reads == 0:
            patterns.append('solved_from_prompt')
        elif reads == 1 and structure >= 1:
            patterns.append('standard_gabb_flow')
        else:
            patterns.append('complex_exploration')

    return patterns
```

**Key insight from 2026-01-01:** Gabb's time savings came from behavioral change (34% of runs solved without reading files) rather than from gabb_structure efficiency per se.

---

## 7. Token Cost Attribution

Understanding *where* tokens are spent helps identify optimization opportunities.

### Token Sources

| Component | Typical Tokens | Notes |
|-----------|----------------|-------|
| Base system prompt | ~22,000 | Cached after first turn |
| SKILL.md content | ~500-700 | Added for Gabb condition |
| gabb_structure output | ~100-150 | Very compact |
| Read file content | 2,000-5,000 | Per file, varies by size |
| Conversation history | ~200-500/turn | Compounds over turns |

### Analyzing Token Breakdown

```python
def analyze_token_sources(transcript_path):
    """Analyze where tokens are spent in a transcript."""
    import json

    sources = {
        'system_prompt': 0,
        'tool_results': 0,
        'assistant_output': 0,
        'user_messages': 0
    }

    with open(transcript_path) as f:
        for line in f:
            msg = json.loads(line)
            msg_type = msg.get('type')
            content = msg.get('message', {}).get('content', '')

            # Rough token estimate: 4 chars per token
            tokens = len(str(content)) // 4

            if msg_type == 'system':
                sources['system_prompt'] += tokens
            elif msg_type == 'tool_result':
                sources['tool_results'] += tokens
            elif msg_type == 'assistant':
                sources['assistant_output'] += tokens
            elif msg_type == 'user':
                sources['user_messages'] += tokens

    return sources
```

### Comparing Conditions

Create a breakdown table:

| Token Type | Control | Gabb | Difference |
|------------|---------|------|------------|
| Cache Read | 57,919 | 73,497 | +27% |
| Cache Create | 1,965 | 868 | -56% |
| Input | 5 | 16 | +232% |
| Output | 546 | 486 | -11% |
| **Total** | 60,435 | 74,867 | **+24%** |
| **Cost (USD)** | $0.03 | $0.03 | **+0%** |

**Key insight:** Higher token counts don't always mean higher costs when caching is effective.

---

## 8. SKILL.md Optimization Analysis (Optional)

**Skip this section if** your benchmark doesn't use a SKILL.md file or you're not investigating token overhead.

If the Gabb condition uses a SKILL.md file, analyze it for redundancy with MCP tool descriptions.

### Measuring SKILL.md Overhead

```python
def analyze_skill_file(skill_path):
    """Break down SKILL.md by section."""
    with open(skill_path) as f:
        content = f.read()

    # Rough token count
    total_tokens = len(content) // 4

    print(f"Total characters: {len(content)}")
    print(f"Estimated tokens: {total_tokens}")

    return total_tokens
```

### Identifying Redundancy

Compare SKILL.md content with MCP tool descriptions. Common duplications:

- "MANDATORY PRE-READ CHECK" directive
- Token cost warnings ("5,000-10,000 tokens")
- Exceptions list (files <50 lines, etc.)
- Usage examples

### Optimization Recommendations

Document potential savings:

| Section | Current Tokens | Redundant? | Proposed |
|---------|----------------|------------|----------|
| Pre-Flight Checklist | ~136 | No | Keep |
| Purpose | ~55 | Yes (in MCP) | Remove |
| When to Use | ~131 | Yes (in MCP) | Remove |
| Supported Languages | ~103 | No | Keep |
| Output Example | ~105 | Useful | Keep |
| **Total** | ~670 | - | ~180 |

---

## 9. Generating a Summary Report

### Complete Analysis Workflow

Follow this checklist for a thorough analysis:

1. **Load and summarize** (Section 1)
   - Load results JSON
   - Extract aggregate statistics for both conditions
   - Calculate raw differences (time, tokens, cost)

2. **Test statistical significance** (Section 5)
   - Run t-test on primary metrics (time, tokens)
   - Calculate confidence intervals
   - Compute effect sizes (Cohen's d)

3. **Analyze subgroups** (Section 6)
   - Group runs by behavior (read count, tool usage)
   - Check for selection bias
   - Compare within subgroups

4. **Attribute token costs** (Section 7)
   - Break down by cache type
   - Identify major token sources
   - Compare cost vs. raw tokens

5. **Generate report** (see template below)
   - Write executive summary
   - Include all tables and statistics
   - Document recommendations

### Manual Analysis Steps

```python
import json, math

# 1. Load results
with open('results_xxx.json') as f:
    data = json.load(f)

control = data['conditions']['control']
gabb = data['conditions']['gabb']

# 2. Extract aggregates
c_agg = control['aggregate']
g_agg = gabb['aggregate']

# 3. Calculate differences
time_diff = g_agg['wall_time_seconds']['mean'] - c_agg['wall_time_seconds']['mean']
time_pct = (time_diff / c_agg['wall_time_seconds']['mean']) * 100

# 4. Statistical test (see Section 5 for full function)
n = c_agg['run_count']
se_diff = math.sqrt(
    (c_agg['wall_time_seconds']['std']**2 + g_agg['wall_time_seconds']['std']**2) / n
)
t_stat = time_diff / se_diff

print(f"Time: {c_agg['wall_time_seconds']['mean']:.1f}s → {g_agg['wall_time_seconds']['mean']:.1f}s")
print(f"Difference: {time_diff:.1f}s ({time_pct:.1f}%)")
print(f"t-statistic: {t_stat:.2f}")

# 5. Subgroup analysis (see Section 6)
for run in gabb['runs']:
    reads = run['tool_calls'].get('Read', 0)
    # ... group and analyze
```

### Creating Detailed Run Tables

Sort individual runs to reveal patterns:

```python
def create_run_table(runs, condition_name):
    """Create sorted table of individual runs."""
    # Sort by time
    sorted_runs = sorted(runs, key=lambda r: r['wall_time_seconds'])

    print(f"\n### {condition_name} Runs (sorted by time)\n")
    print("| # | Time | Tokens | Reads | Structure | Success |")
    print("|---|------|--------|-------|-----------|---------|")

    for i, run in enumerate(sorted_runs, 1):
        time = run['wall_time_seconds']
        tokens = run['tokens_total']
        reads = run['tool_calls'].get('Read', 0)
        structure = run['tool_calls'].get('mcp__gabb__gabb_structure', 0)
        success = '✓' if run['success'] else '✗'

        print(f"| {i} | {time:.1f}s | {tokens:,} | {reads} | {structure} | {success} |")
```

### Report Template

Create analysis files in `benchmark/claude-code/analysis/YYYY-MM-DD.md`:

```markdown
# Benchmark Analysis: YYYY-MM-DD

## Executive Summary

- Key finding 1
- Key finding 2
- Key finding 3

## 1. Aggregate Results

### Primary Metrics

| Metric | Control | Gabb | Difference |
|--------|---------|------|------------|
| Success Rate | X% | Y% | - |
| Time (s) | {mean} ± {std} | {mean} ± {std} | {diff} ({%}) |
| Total Tokens | {mean} ± {std} | {mean} ± {std} | {diff} ({%}) |
| Cost (USD) | ${mean} ± ${std} | ${mean} ± ${std} | {diff} ({%}) |

### Cache Token Breakdown

| Token Type | Control | Gabb | Difference |
|------------|---------|------|------------|
| Cache Read | X | Y | +Z% |
| Cache Create | X | Y | -Z% |
| Output | X | Y | -Z% |

## 2. Statistical Significance

[t-test results and confidence intervals]

## 3. Subgroup Analysis

[Breakdown by read count, gabb_structure usage]

## 4. Behavioral Patterns

[Identify distinct patterns and their frequencies]

## 5. Token Cost Attribution

[Where tokens are spent]

## 6. Recommendations

[Actionable next steps]

## Appendix: Raw Data Location

- Results JSON: `benchmark/claude-code/results/results_xxx.json`
- Transcripts: `~/.claude/projects/{path}/*.jsonl`
```

---

## 10. Troubleshooting Analysis

### No Transcripts Found

- Check the exact workspace path used in the benchmark
- Transcripts are only created for interactive sessions
- Background/headless runs may not persist transcripts

### High Variance in Results

- Run more iterations (n=20 minimum, n=50 for subgroups)
- Check for outliers in individual runs
- Consider excluding failed runs from aggregate
- Look for bimodal distributions (may indicate two distinct behaviors)

### Gabb Performing Worse

Check these potential issues:
1. **SKILL.md not loaded** - Verify skill file is present in config
2. **Index not ready** - Daemon may not have finished indexing
3. **Wrong tools used** - Review transcripts for tool selection patterns
4. **Complex task** - Some tasks may not benefit from semantic search
5. **Selection bias** - Compare within subgroups, not across all runs

### Comparing Multiple Task Results

For suite results (`suite_results_*.json`), use the `summary` key for aggregate stats or iterate over `tasks[]`:

```python
# Load suite results
with open('suite_results_n20_xxx.json') as f:
    suite = json.load(f)

# Use pre-computed summary (recommended)
control = suite['summary']['control']
gabb = suite['summary']['gabb']
print(f"Time: {control['wall_time_seconds']['mean']:.1f}s → {gabb['wall_time_seconds']['mean']:.1f}s")

# Or iterate per-task (note: different structure than individual results!)
for task in suite['tasks']:
    task_id = task['task_id']
    # Tasks have 'control' and 'gabb' keys directly, not 'conditions'
    c_runs = task['control']['runs']
    g_runs = task['gabb']['runs']
```

### Structure Mismatch Errors

**Problem:** Scripts fail with `KeyError: 'conditions'` or `KeyError: 'aggregate'`

**Cause:** Suite and individual results have different structures:

| Access Pattern | Individual Task | Suite |
|----------------|-----------------|-------|
| Aggregate stats | `data['conditions']['control']['aggregate']` | `data['summary']['control']` |
| Per-task data | N/A | `data['tasks'][i]['control']` |
| Runs array | `data['conditions']['control']['runs']` | `data['summary']` doesn't have runs (use per-task) |

**Solution:** Use `analyze.py` which auto-detects the format:

```bash
python benchmark/claude-code/analyze.py --latest
```

Or detect manually:

```python
def get_aggregate_stats(data, condition='control'):
    """Get aggregate stats regardless of result type."""
    if 'summary' in data:  # Suite
        return data['summary'][condition]
    elif 'conditions' in data:  # Individual
        return data['conditions'][condition].get('aggregate', data['conditions'][condition])
    raise ValueError("Unknown format")
```

---

## 11. Best Practices

### Running Benchmarks

1. **Always run both conditions** - Control provides the baseline
2. **Use sufficient runs** - n=10 quick check, n=20 publication, n=50 subgroups
3. **Document the environment** - Claude model version, gabb version, date
4. **Save raw transcripts** - For deeper analysis later

### Analyzing Results

5. **Always test significance** - Raw differences mean nothing without p-values
6. **Watch for selection bias** - Compare within subgroups when appropriate
7. **Report both tokens AND cost** - Caching changes the economics
8. **Create detailed run tables** - Aggregates hide important patterns

### Documentation

9. **Update README.md** - Keep current results visible
10. **Create dated analysis files** - `analysis/YYYY-MM-DD.md`
11. **Archive old results** - Don't delete, move to dated folders

---

## 12. Quick Analysis Commands

### Using analyze.py (Recommended)

The unified analysis script handles both individual and suite results:

```bash
# Analyze latest benchmark (auto-detects type)
python benchmark/claude-code/analyze.py --latest

# Analyze specific file
python benchmark/claude-code/analyze.py results/suite_results_*.json

# Output as markdown
python benchmark/claude-code/analyze.py --latest --markdown

# Output as JSON (for further processing)
python benchmark/claude-code/analyze.py --latest --json

# Find results from a specific date
python benchmark/claude-code/analyze.py --date 20260104

# Save markdown report to analysis/ directory
python benchmark/claude-code/analyze.py --latest --markdown --save
```

**Features:**
- Auto-detects suite vs individual task results
- Calculates t-tests, confidence intervals, Cohen's d
- Shows significance stars (*, **, ***) for p < 0.05, 0.01, 0.001
- Works with any condition names (not just control/gabb)

### Legacy Manual Commands

```bash
# Show latest results file
ls -t benchmark/claude-code/results/*.json | head -1

# Pretty-print latest results
cat $(ls -t benchmark/claude-code/results/*.json | head -1) | python3 -m json.tool | head -100

# Count recent benchmark transcripts
find ~/.claude/projects -name "*.jsonl" -mtime -1 | wc -l

# Analyze with telemetry tools
cd benchmark/telemetry && gabb-benchmark analyze ~/.claude/projects/{path}/*.jsonl
```
