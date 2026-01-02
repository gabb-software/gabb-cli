# Benchmark Analysis

Analyze Claude Code benchmark results with interactive filtering.

## Step 1: Discover Available Data

First, list available results files with their timestamps:

```bash
ls -lt benchmark/claude-code/results/*.json 2>/dev/null | head -20
```

Also check for any existing analyses:

```bash
ls -lt benchmark/claude-code/analysis/*.md 2>/dev/null | head -5
```

## Step 2: Ask What to Analyze

Use the AskUserQuestion tool to gather requirements:

**Question 1: Which results to analyze?**
- Latest results file (Recommended)
- All results from today
- Results after a specific time (will prompt for time)
- Specific file (will prompt for filename)

**Question 2: Analysis depth?**
- Quick summary - aggregates and basic comparison only
- Standard analysis - includes statistical tests and subgroups (Recommended)
- Deep dive - full methodology with token attribution and detailed run tables

**Question 3: Output format?**
- Terminal only - display results without saving
- Create analysis file - save to `benchmark/claude-code/analysis/YYYY-MM-DD.md` (Recommended)
- Both - display and save

## Step 3: Filter Results (if needed)

If user selected time-based filtering:

```bash
# Files modified after a specific time today
find benchmark/claude-code/results -name "*.json" -newermt "today HH:MM"

# Or for a specific date/time
find benchmark/claude-code/results -name "*.json" -newermt "YYYY-MM-DD HH:MM"
```

Parse the timestamp from filenames (format: `results_*_YYYYMMDD_HHMMSS.json`) if modification time is unreliable.

## Step 4: Load and Analyze

For each selected results file:

### 4a. Load Data
```python
import json
with open('results_file.json') as f:
    data = json.load(f)
control = data['conditions']['control']
gabb = data['conditions']['gabb']
```

### 4b. Quick Summary (always)
Calculate and display:
- Success rates for both conditions
- Time: mean ± std, difference, percentage change
- Tokens: mean ± std, difference, percentage change
- Cost: mean ± std, difference (if available)
- Tool calls: mean ± std for key tools (Read, Grep, Glob, gabb_structure)

### 4c. Statistical Analysis (Standard or Deep)
Follow Section 5 of `benchmark/claude-code/RESULTS_ANALYSIS_GUIDE.md`:
- Two-sample t-test for time and tokens
- Calculate confidence intervals
- Compute Cohen's d effect size
- Interpret significance levels

### 4d. Subgroup Analysis (Standard or Deep)
Follow Section 6 of the guide:
- Group runs by Read count (0, 1, 2+)
- Check gabb_structure usage rate within each group
- Identify behavioral patterns
- Warn about selection bias if comparing across groups

### 4e. Token Attribution (Deep only)
Follow Section 7 of the guide:
- Break down by cache type (read, create, input, output)
- Calculate actual cost vs raw token count
- Identify major token sources

### 4f. Detailed Run Tables (Deep only)
Follow Section 9 of the guide:
- Create sorted tables of individual runs
- Highlight patterns and outliers

## Step 5: Generate Output

### Terminal Output
Display a formatted summary with:
- Key metrics table
- Statistical significance indicators (*, **, ***)
- Effect size interpretation
- Key insights (1-3 bullet points)

### Analysis File (if requested)
Create `benchmark/claude-code/analysis/YYYY-MM-DD.md` following the template in Section 9 of the guide:
- Executive summary
- Aggregate results with cache breakdown
- Statistical significance section
- Subgroup analysis
- Recommendations
- Appendix with raw data locations

## Step 6: Follow-up

After presenting results, ask if the user wants:
- Deeper analysis on any specific metric
- Comparison with a previous analysis
- To investigate specific runs or transcripts
- Recommendations documented or actioned

## Reference

Full methodology: `benchmark/claude-code/RESULTS_ANALYSIS_GUIDE.md`

Key sections:
- Section 5: Statistical Significance Testing
- Section 6: Subgroup Analysis & Selection Bias
- Section 7: Token Cost Attribution
- Section 9: Generating a Summary Report
