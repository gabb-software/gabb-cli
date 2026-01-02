# Benchmark Results Analysis Guide

This guide explains how to analyze Claude Code benchmark results, including reading transcripts, interpreting metrics, and producing summary reports.

## Quick Reference

| What You Need | Where To Find It |
|--------------|------------------|
| Benchmark results | `benchmark/claude-code/results/*.json` |
| Claude transcripts | `~/.claude/projects/{workspace-path}/*.jsonl` |
| Telemetry tools | `benchmark/telemetry/` |
| Current summary | `benchmark/claude-code/README.md` |

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

### Per-Run Metrics

Each run in the `runs` array contains:

| Field | Description |
|-------|-------------|
| `wall_time_seconds` | Total execution time |
| `tokens_input` | Input tokens consumed |
| `tokens_output` | Output tokens generated |
| `tokens_total` | Sum of input + output |
| `tool_calls` | Map of tool name → call count |
| `tool_calls_total` | Total number of tool invocations |
| `turns` | Conversation turns |
| `success` | Whether task was completed correctly |
| `final_answer` | Claude's final response |

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

## 4. Key Metrics to Compare

### Primary Metrics

| Metric | What It Shows | Target |
|--------|---------------|--------|
| **Success Rate** | Task completion accuracy | Higher is better |
| **Time (seconds)** | Wall-clock execution time | Lower is better |
| **Total Tokens** | Cost/efficiency indicator | Lower is better |
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

1. **Token Reduction**: Gabb should use fewer tokens due to:
   - Semantic search vs. text search
   - Targeted file reads vs. full file reads
   - Fewer navigation iterations

2. **Tool Call Reduction**: Gabb should make fewer calls:
   - Direct symbol lookup vs. grep→read→grep cycles
   - Structure preview before reading

3. **Time Improvement**: Faster completion due to:
   - More direct navigation path
   - Less trial-and-error searching

4. **Consistent Success**: Both conditions should succeed equally

## 5. Generating a Summary Report

### Manual Analysis Steps

1. **Load the results file**:
   ```python
   import json
   with open('results_xxx.json') as f:
       data = json.load(f)
   ```

2. **Extract aggregates**:
   ```python
   control = data['conditions']['control']['aggregate']
   gabb = data['conditions']['gabb']['aggregate']
   ```

3. **Calculate differences**:
   ```python
   time_diff = gabb['wall_time_seconds']['mean'] - control['wall_time_seconds']['mean']
   time_pct = (time_diff / control['wall_time_seconds']['mean']) * 100
   ```

### Report Template

Update `benchmark/claude-code/README.md` with:

```markdown
## Current Benchmark

### Results: {task_id} ({n} runs)

| Metric       | Control          | Gabb             | Diff          |
|--------------|------------------|------------------|---------------|
| Success      | {control_%}%     | {gabb_%}%        |               |
| Time (s)     | {mean} ± {std}   | {mean} ± {std}   | {diff} ({%})  |
| Total Tokens | {mean} ± {std}   | {mean} ± {std}   | {diff} ({%})  |
| Tool Calls   | {mean} ± {std}   | {mean} ± {std}   | {diff}        |
| Turns        | {mean} ± {std}   | {mean} ± {std}   | {diff}        |

### Tool Usage

| Tool                      | Control   | Gabb      |
|---------------------------|-----------|-----------|
| Glob                      | {m} ± {s} | {m} ± {s} |
| Grep                      | {m} ± {s} | {m} ± {s} |
| Read                      | {m} ± {s} | {m} ± {s} |
| mcp__gabb__gabb_structure | 0.0 ± 0.0 | {m} ± {s} |
| mcp__gabb__gabb_symbols   | 0.0 ± 0.0 | {m} ± {s} |
| Bash                      | {m} ± {s} | {m} ± {s} |
| Task                      | {m} ± {s} | {m} ± {s} |
```

## 6. Troubleshooting Analysis

### No Transcripts Found

- Check the exact workspace path used in the benchmark
- Transcripts are only created for interactive sessions
- Background/headless runs may not persist transcripts

### High Variance in Results

- Run more iterations (n=20 recommended for stable statistics)
- Check for outliers in individual runs
- Consider excluding failed runs from aggregate

### Gabb Performing Worse

Check these potential issues:
1. **SKILL.md not loaded** - Verify skill file is present in config
2. **Index not ready** - Daemon may not have finished indexing
3. **Wrong tools used** - Review transcripts for tool selection patterns
4. **Complex task** - Some tasks may not benefit from semantic search

### Comparing Multiple Task Results

For suite results (`suite_results_*.json`), aggregate across tasks:

```python
# Load suite results
with open('suite_results_n20_xxx.json') as f:
    suite = json.load(f)

# Aggregate metrics across all tasks
for task_id, task_data in suite['tasks'].items():
    control = task_data['conditions']['control']['aggregate']
    gabb = task_data['conditions']['gabb']['aggregate']
    # ... compute comparisons
```

## 7. Best Practices

1. **Always run both conditions** - Control provides the baseline
2. **Use sufficient runs** - n=10 minimum, n=20 for publication
3. **Document the environment** - Claude model version, gabb version
4. **Save raw transcripts** - For deeper analysis later
5. **Update README.md** - Keep current results visible
6. **Archive old results** - Don't delete, move to dated folders

## 8. Quick Analysis Commands

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
