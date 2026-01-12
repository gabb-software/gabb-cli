# Claude Code Benchmark Results

This document presents the results from gabb's Claude Code benchmark suite, demonstrating the measurable benefits of semantic code indexing for AI-assisted code navigation.

## Executive Summary

Gabb delivers **32% faster task completion** with **38% fewer tool calls** while maintaining equivalent success rates. These results are statistically significant with very high confidence (p<0.001).

| Metric | Without Gabb | With Gabb | Improvement |
|--------|--------------|-----------|-------------|
| **Task Completion Time** | 45.4s | 30.6s | **32.5% faster** |
| **Tool Calls per Task** | 14.6 | 8.1 | **45% fewer** |
| **Success Rate** | 94% | 95% | +1% |
| **Cost** | $0.040 | $0.040 | Equal |

---

## Why This Benchmark Matters

### The Problem: Inefficient Code Navigation

When AI assistants navigate unfamiliar codebases, they typically rely on text-based search tools like `grep` and file reads. This approach suffers from:

1. **Trial and error**: Multiple search attempts to find the right files
2. **Excessive file reads**: Reading entire files when only specific functions are needed
3. **Wasted time**: Each tool call adds latency to the overall task

### The Solution: Semantic Indexing

Gabb pre-indexes code structure (functions, classes, methods) into a local SQLite database. This enables:

1. **Direct navigation**: Jump to symbols by name without searching
2. **Targeted reads**: Preview file structure before reading content
3. **Fewer round-trips**: Less back-and-forth between search and read operations

### Industry-Standard Evaluation

This benchmark uses tasks derived from **SWE-bench**, the industry-standard benchmark for evaluating AI coding assistants. SWE-bench tasks represent real GitHub issues from popular open-source projects, making our results directly relevant to everyday developer workflows.

---

## Detailed Results

### Latest Benchmark Run

**Suite**: `main @ 554bfda` | January 11, 2026
**Scale**: 40 SWE-bench lite tasks × 10 runs each = 400 total runs per condition
**Model**: Claude Sonnet (via Claude Code CLI)

### Primary Metrics

| Metric | Control (no gabb) | With Gabb | Difference | Significance |
|--------|-------------------|-----------|------------|--------------|
| Wall Time | 45.4s ± 43.3s | 30.6s ± 34.9s | -14.8s (-32.5%) | *** |
| Tool Calls | 14.6 ± 18.2 | 8.1 ± 13.5 | -6.5 (-44.5%) | *** |
| Success Rate | 94% (376/400) | 95% (380/400) | +1% | n/a |
| Turns | 3.6 ± 2.8 | 4.0 ± 3.1 | +0.4 | ns |

_Significance: \* p < 0.05, \*\* p < 0.01, \*\*\* p < 0.001_

### Tool Usage Comparison

The benchmark reveals a fundamental shift in how Claude navigates code when gabb is available:

| Tool | Control (avg/run) | Gabb (avg/run) | Reduction |
|------|-------------------|----------------|-----------|
| Read | 5.1 | 3.2 | **37%** |
| Grep | 3.9 | 2.4 | **38%** |
| Bash | 3.9 | 1.4 | **65%** |
| Glob | 1.1 | 0.5 | **58%** |
| Task (subagent) | 0.6 | 0.2 | **67%** |
| gabb_symbol | — | 0.5 | _new_ |
| gabb_structure | — | 0.3 | _new_ |

**Key insight**: Gabb replaces the "grep_then_read" pattern (74.8% of control runs) with direct "symbol_search" (39.2%) and "direct_read" (44.0%) patterns, reducing total tool invocations by 45%.

### Token Usage & Cost

| Metric | Control | Gabb | Difference |
|--------|---------|------|------------|
| Total Tokens | 79,313 ± 116k | 86,748 ± 79k | +9.4% |
| Output Tokens | 701 | 654 | -6.7% |
| **Cost (USD)** | $0.040 | $0.040 | **0%** |

**Cost-neutral despite token increase**: The 9.4% token increase (from SKILL.md in system prompt) is fully offset by prompt caching, resulting in identical per-run costs. The 32% time savings come at zero additional API cost.

---

## Statistical Analysis

### Significance Testing

We use Welch's t-test to determine if observed differences are statistically significant:

**Wall Time Reduction**
- Mean difference: 14.8 seconds
- Standard error: 2.79 seconds
- t-statistic: **5.31**
- p-value: **< 0.001**
- 95% Confidence Interval: **[9.2s, 20.3s]**

This means we can be 95% confident that gabb saves between 9 and 20 seconds per task.

### Effect Size

Cohen's d measures the practical significance of the improvement:

| Metric | Cohen's d | Interpretation |
|--------|-----------|----------------|
| Wall Time | **0.38** | Small-medium effect |
| Tool Calls | **0.41** | Small-medium effect |

A Cohen's d of 0.4 represents a meaningful effect size—the improvement is consistent enough to be practically valuable across diverse tasks.

### Sample Size & Power

With 400 runs per condition (40 tasks × 10 repetitions), this benchmark has sufficient statistical power to:

- Detect small-to-medium effect sizes (d > 0.25) with 99% confidence
- Distinguish real improvements from random variance
- Support detailed subgroup analysis by task complexity and project

---

## Testing Methodology

### A/B Testing Design

Each benchmark run executes tasks under two conditions:

1. **Control**: Standard Claude Code with built-in tools (Grep, Read, Glob, Bash)
2. **Gabb**: Claude Code with gabb MCP server enabled + SKILL.md guidance

Both conditions receive identical prompts and success criteria. The only difference is tool availability.

### Task Selection

Tasks are drawn from **SWE-bench Lite**, a curated subset of real GitHub issues:

- **40 diverse tasks** from Django (34) and Astropy (6)
- **Real bug fixes**: Each task requires locating and fixing actual GitHub issues
- **Ground truth validation**: Expected patches are known, enabling automated success checking

Example task:
> "Fix the migration issue in Django where proxy model permissions are not created correctly when using a custom User model."

### Execution Environment

- **Isolation**: Each task runs in a fresh git checkout at a specific commit
- **Indexing**: Gabb daemon indexes the workspace before task execution
- **Hooks**: PostToolUse hooks log every tool call for analysis
- **Repetition**: 10 runs per task reduces variance from LLM non-determinism

### Metrics Collection

For each run, we capture:
- Wall-clock execution time
- Token consumption (input, output, cache)
- Tool calls by type
- Conversation turns
- Success/failure status
- Final answer for manual review

---

## How Gabb Improves Performance

### Before: Search-Read-Search Cycles

Without gabb, Claude must iteratively search for code:

```
1. Grep for "Blueprint" → 47 matches across 23 files
2. Read flask/app.py → not here
3. Grep for "class Blueprint" → 3 matches
4. Read flask/blueprints.py → found it!
```

**Result**: 4+ tool calls, ~45 seconds

### After: Direct Navigation

With gabb, Claude can query the symbol index directly:

```
1. gabb_structure("flask/blueprints.py") → shows Blueprint class at line 42
2. Read flask/blueprints.py:40-100 → confirm implementation
```

**Result**: 2 tool calls, ~30 seconds

### Why It Works

1. **Semantic understanding**: Gabb indexes code by meaning (functions, classes, methods), not just text
2. **Pre-computation**: Indexing happens once; queries are instant
3. **Targeted reads**: Structure previews enable reading only relevant sections
4. **Reduced uncertainty**: Claude spends less time exploring and more time confirming

---

## Benefits Summary

### For Developers

| Benefit | Impact |
|---------|--------|
| **Faster responses** | 32% reduction in wait time |
| **Lower latency** | 45% fewer tool calls |
| **Same accuracy** | 95% success rate maintained |
| **No extra cost** | Identical cost per task |

### For Teams

| Benefit | Impact |
|---------|--------|
| **Higher throughput** | Complete more tasks per hour |
| **Consistent performance** | Less variance in response times |
| **Scalable** | Works on large codebases (tested on repos with 1M+ LOC) |

### Technical Advantages

| Advantage | Description |
|-----------|-------------|
| **Local-first** | No code leaves your machine |
| **Language-aware** | Supports Python, TypeScript, Rust, Kotlin, C++ |
| **Incremental** | File watcher updates index in real-time |
| **Lightweight** | SQLite database, minimal resource usage |

---

## Running the Benchmark

### Prerequisites

- Python 3.9+
- Claude Code CLI installed
- gabb binary in PATH

### Quick Start

```bash
cd benchmark/claude-code

# Run a single SWE-bench task
python run.py --swe-bench django__django-11179 --runs 5

# Run the full benchmark suite
python run.py --swe-bench-suite --limit 20 --runs 10

# Run with concurrent execution
python run.py --swe-bench-suite --limit 20 --runs 10 --concurrent 3

# Analyze results
python analyze.py --latest --markdown
```

### Output

Results are saved to `results/suite_results_*.json` with full metrics for each run.

---

## Appendix: Raw Data

### Results File Location

```
benchmark/claude-code/results/suite_results_main_554bfda_n10_20260111_205711.json
```

### Analysis Reports

Detailed analysis with task classifications available at:
```
benchmark/claude-code/analysis/2026-01-11.md
```

### Analysis Tools

```bash
# Generate analysis report
python analyze.py --latest --markdown --save

# View tool usage breakdown
python compare.py results/suite_results_*.json
```

### Reproducing Results

```bash
# Clone and setup
git clone https://github.com/gabb-software/gabb-cli
cd gabb-cli
cargo build --release

# Run benchmark
cd benchmark/claude-code
python run.py --swe-bench-suite --limit 40 --runs 10
```

---

_Last updated: January 11, 2026_
_Benchmark version: main @ 554bfda_
