# Gabb Benchmark Suite

Two complementary benchmarks for evaluating gabb's code navigation capabilities.

## Benchmarks Overview

| Benchmark | Question Answered | How It Works |
|-----------|-------------------|--------------|
| **API Benchmark** (`api/`) | Is semantic indexing more efficient than grep? | Direct Anthropic API with custom tool definitions |
| **Claude Code Benchmark** (`claude-code/`) | Does Claude Code choose gabb over Grep/Read? | Runs Claude Code CLI with/without gabb MCP |

### Why Two Benchmarks?

The **API benchmark** isolates the core value proposition: given identical LLM capabilities, does semantic indexing reduce tokens and time vs text search?

The **Claude Code benchmark** tests real-world UX: with SKILL.md guidance, does Claude Code actually prefer gabb tools, and does that improve outcomes?

If gabb wins in API but loses in Claude Code, that indicates a SKILL.md or MCP integration issue.
If gabb loses in both, that's a fundamental capability gap.

---

## Claude Code Benchmark (`claude-code/`)

Tests Claude Code's tool selection behavior with and without gabb.

### Quick Start

```bash
cd benchmark/claude-code

# List available tasks
python run.py --list-tasks

# Run a single task comparison
python run.py --task sklearn-ridge-normalize --workspace /path/to/sklearn-repo

# Run just one condition
python run.py --task sklearn-ridge-normalize --workspace /path/to/sklearn-repo --condition gabb
```

### What It Measures

| Metric | Description |
|--------|-------------|
| Wall-clock time | Total seconds to complete task |
| Tokens (input/output) | Token consumption |
| Tool calls by type | Grep, Read, Glob, mcp__gabb__* counts |
| Success | Did Claude find the correct file? |

### How It Works

1. **Control condition**: Standard Claude Code (Grep, Read, Glob tools)
2. **Gabb condition**: Claude Code with gabb MCP server + SKILL.md

Both conditions receive the same prompt. A PostToolUse hook logs every tool call.

### Output

```
Results: sklearn-ridge-normalize
┌──────────────┬─────────┬─────────┬──────────────────┐
│ Metric       │ Control │    Gabb │             Diff │
├──────────────┼─────────┼─────────┼──────────────────┤
│ Success      │    PASS │    PASS │                  │
│ Time (s)     │    45.2 │    32.1 │  -13.1 (-29%)    │
│ Total Tokens │  52,340 │  38,210 │ -14,130 (-27%)   │
│ Tool Calls   │      18 │      11 │              -7  │
└──────────────┴─────────┴─────────┴──────────────────┘

Tool Usage Breakdown:
┌────────────────────────────┬─────────┬──────┐
│ Tool                       │ Control │ Gabb │
├────────────────────────────┼─────────┼──────┤
│ Grep                       │       8 │    2 │
│ Read                       │       7 │    4 │
│ mcp__gabb__gabb_symbols    │       0 │    3 │
│ mcp__gabb__gabb_structure  │       0 │    2 │
└────────────────────────────┴─────────┴──────┘
```

### Adding Tasks

Edit `tasks/tasks.json`:

```json
{
  "tasks": [
    {
      "id": "my-task-id",
      "repo": "owner/repo",
      "prompt": "Find the file that...",
      "expected_files": ["path/to/expected.py"]
    }
  ]
}
```

---

## API Benchmark (`api/`)

Tests gabb CLI directly via Anthropic API in Docker containers.

### Quick Start

```bash
cd benchmark/api

# Setup (builds gabb binary, pulls Docker images)
python setup.py

# Configure API key
cp .env.example .env
# Edit .env: ANTHROPIC_API_KEY=your-key

# Run single task
python run.py --task scikit-learn__scikit-learn-10297

# Run multiple tasks
python run.py --tasks 20 --concurrent 5
```

### Architecture

```
api/
├── core/
│   ├── dataset.py    # SWE-bench data loader
│   ├── env.py        # Docker environment wrapper
│   ├── agent.py      # Control vs Gabb agents
│   └── tools.py      # Tool definitions
├── bin/gabb          # Linux binary (built by setup.py)
├── results/          # Output CSV/JSON
└── run.py            # Main runner
```

### Agents

| Agent | Tools |
|-------|-------|
| **Control** | grep, find_file, read_file, bash |
| **Gabb** | gabb_symbols, gabb_definition, gabb_structure, gabb_usages, read_file |

### Metrics

| Metric | Description |
|--------|-------------|
| tokens_input/output | Token consumption |
| turns | Conversation turns |
| tool_calls | Total + per-tool breakdown |
| time_seconds | Wall-clock time |
| success | Found correct file? |

---

## Development

### Requirements

- Python 3.9+
- Docker (for API benchmark)
- Claude Code CLI (for Claude Code benchmark)
- gabb binary

### Install Dependencies

```bash
# For API benchmark
cd api && pip install -r requirements.txt

# For Claude Code benchmark
cd claude-code && pip install rich  # optional, for pretty output
```

### Adding New Benchmarks

Follow the pattern:
1. Create condition configs in `configs/`
2. Define tasks in `tasks/`
3. Implement runner with metrics collection
4. Output to `results/`
