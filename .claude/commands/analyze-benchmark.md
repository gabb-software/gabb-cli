# Benchmark Analysis

Analyze Claude Code benchmark results using the unified analysis script.

## Step 1: Run Initial Analysis

Start by running the analyze.py script on the latest results:

```bash
python benchmark/claude-code/analyze.py --latest
```

This will:
- Auto-detect if it's a suite or individual task result
- Show primary metrics (time, tokens, success rate)
- Calculate statistical significance (t-tests, Cohen's d)
- Display tool usage comparison

## Step 2: Ask About Output Preferences

Use AskUserQuestion to determine output format:

**Question: How should results be saved?**
- Terminal only - just display (Recommended for quick checks)
- Save markdown report - create `analysis/YYYY-MM-DD.md`
- Both - display and save

If saving, run:
```bash
python benchmark/claude-code/analyze.py --latest --markdown --save
```

## Step 3: Interpret Results

### Key Metrics to Highlight

After running the analysis, summarize the findings:

1. **Statistical Significance** - Look for stars:
   - `*` = p < 0.05 (likely real effect)
   - `**` = p < 0.01 (strong evidence)
   - `***` = p < 0.001 (very strong evidence)

2. **Effect Size** - Cohen's d interpretation:
   - Small (0.2): Detectable but modest improvement
   - Medium (0.5): Noticeable practical improvement
   - Large (0.8+): Substantial improvement

3. **Tool Usage Changes** - Compare:
   - Read/Grep/Glob reduction (positive for gabb)
   - gabb_structure/gabb_symbols usage (expected in gabb condition)

### Red Flags to Watch For

- Success rate difference between conditions
- Token increase without time savings
- High variance (std > 50% of mean)

## Step 4: Additional Analysis (Optional)

For deeper investigation:

### Analyze Specific File
```bash
python benchmark/claude-code/analyze.py results/suite_results_*.json
```

### JSON Output for Processing
```bash
python benchmark/claude-code/analyze.py --latest --json > analysis.json
```

### Find Results by Date
```bash
python benchmark/claude-code/analyze.py --date 20260104
```

## Step 5: Follow-up Questions

After presenting results, offer:
- Comparison with previous analyses
- Investigation of specific runs or transcripts
- Review of per-task breakdown (for suite results)
- Recommendations for next steps

## Quick Reference

| Command | Purpose |
|---------|---------|
| `--latest` | Analyze most recent results file |
| `--markdown` | Output as markdown |
| `--json` | Output as JSON |
| `--save` | Save report to analysis/ |
| `--date YYYYMMDD` | Filter by date |

## Reference Documentation

- Full guide: `benchmark/claude-code/RESULTS_ANALYSIS_GUIDE.md`
- Script source: `benchmark/claude-code/analyze.py`
- Past analyses: `benchmark/claude-code/analysis/*.md`
