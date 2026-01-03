# A/B Branch Testing

Run A/B benchmark tests comparing gabb performance across git branches.

## Step 1: Discover Available Branches

First, list available git branches:

```bash
git branch -a --sort=-committerdate | head -20
```

Also check current branch and working tree status:

```bash
git status --short
git rev-parse --abbrev-ref HEAD
```

## Step 2: Ask Which Branches to Compare

Use the AskUserQuestion tool to gather branch selection:

**Question 1: Baseline branch (Branch A)?**
- `main` - The main branch (Recommended)
- Current branch - Whatever branch is currently checked out
- Other - Will prompt for branch name

**Question 2: Comparison branch (Branch B)?**
- Current branch - The branch currently checked out (if different from baseline)
- Most recent feature branch - Based on commit date
- Other - Will prompt for branch name

## Step 3: Ask What Benchmark to Run

Use AskUserQuestion to configure the benchmark:

**Question 1: Which benchmark task?**
- SWE-bench task - Use a task from SWE-bench dataset (Recommended for realistic testing)
- Manual task - Use a task from tasks.json with explicit workspace

**Question 2 (if SWE-bench): Which SWE-bench task?**
Suggest these commonly-used tasks:
- `scikit-learn__scikit-learn-10297` - sklearn Ridge normalize parameter (Recommended - fast, reliable)
- `astropy__astropy-14995` - Astropy units handling
- `django__django-11099` - Django URL routing
- Other - Will prompt for task ID

**Question 2 (if Manual): Which manual task?**
List available tasks from:
```bash
cat benchmark/claude-code/tasks/tasks.json | python3 -c "import json,sys; tasks=json.load(sys.stdin); print('\n'.join(t['id'] for t in tasks))"
```

**Question 3: How many runs per condition?**
- 1 run - Quick sanity check
- 3 runs - Fast comparison with some variance data
- 5 runs - Balanced speed vs statistical power (Recommended)
- 10 runs - Higher statistical confidence

**Question 4: Which conditions to test?**
- Both control and gabb - Full A/B comparison (Recommended)
- Gabb only - Just test with gabb enabled
- Control only - Just test without gabb

**Question 5: Include statistical tests?**
- Yes - Include Mann-Whitney U test and p-values (Recommended if runs >= 3)
- No - Just show raw metrics and deltas

## Step 4: Check Prerequisites

Before running, verify:

```bash
# Check if working tree is clean
git status --porcelain

# Verify both branches exist
git rev-parse --verify <branch-a> 2>/dev/null && echo "Branch A exists"
git rev-parse --verify <branch-b> 2>/dev/null && echo "Branch B exists"

# Check cargo is available
cargo --version
```

If working tree is dirty, warn user and ask if they want to:
- Stash changes automatically (use --force)
- Abort and let them handle it manually

## Step 5: Run the A/B Test

Execute the ab_test.py script with gathered parameters:

```bash
cd benchmark/claude-code
python3 ab_test.py \
  --branch-a <baseline> \
  --branch-b <comparison> \
  --swe-bench <task-id> \
  --runs <count> \
  --condition <both|gabb|control> \
  [--stats] \
  [--force]
```

For manual tasks:
```bash
python3 ab_test.py \
  --branch-a <baseline> \
  --branch-b <comparison> \
  --task <task-id> \
  --workspace <path> \
  --runs <count> \
  [--stats] \
  [--force]
```

## Step 6: Present Results

The script will output a markdown comparison report. After completion:

1. Summarize key findings:
   - Which branch performed better
   - Percentage improvements in time, tokens, cost
   - Statistical significance (if --stats was used)

2. Highlight notable tool usage changes:
   - Did gabb_structure usage increase?
   - Did Read/Grep usage decrease?

3. Note any issues or anomalies:
   - Build failures on either branch
   - Significant variance in results
   - Unexpected tool behavior

## Step 7: Follow-up Options

Ask if the user wants to:
- Run more iterations for higher confidence
- Test a different task for broader validation
- Save the comparison to analysis/ directory
- Push results to a branch for CI tracking

## Reference

Related files:
- `benchmark/claude-code/ab_test.py` - A/B workflow automation
- `benchmark/claude-code/compare.py` - Result comparison tool
- `benchmark/claude-code/run.py` - Core benchmark runner
- `benchmark/claude-code/RESULTS_ANALYSIS_GUIDE.md` - Full analysis methodology
