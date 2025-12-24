# Gabb Benchmark Suite

A modular benchmarking suite to evaluate `gabb-cli` (semantic code indexing) vs traditional tools (`grep`/`find`/`read`) for code navigation tasks.

## Hypothesis

Agents using gabb's semantic indexing will find relevant source code files **faster** and with **less token overhead** than agents using traditional text-search tools.

## Quick Start

### 1. Setup

```bash
cd benchmark

# Full automated setup (builds gabb, pulls Docker images, installs deps)
python setup.py

# Verify setup
python setup.py --verify-only
```

### 2. Configure API Key

```bash
# Copy the example and add your API key
cp .env.example .env
# Edit .env and set ANTHROPIC_API_KEY=your-key-here
```

### 3. Run Benchmark

```bash
# Phase 1: Single task test
python run.py --task scikit-learn__scikit-learn-10297

# Phase 2: Multiple tasks
python run.py --tasks 20 --concurrent 5
```

## Architecture

```
benchmark/
├── core/
│   ├── dataset.py    # SWE-bench data loader and patch parser
│   ├── env.py        # Docker environment wrapper
│   ├── agent.py      # BaseAgent, ControlAgent, GabbAgent
│   └── tools.py      # Tool definitions (grep, find, gabb_symbols, etc.)
├── bin/
│   └── gabb          # Linux binary (built by setup.py)
├── results/          # Benchmark results (CSV, JSON)
├── setup.py          # Automated setup script
├── run.py            # Main benchmark runner
└── README.md
```

## Components

### Dataset (`core/dataset.py`)

Loads the `princeton-nlp/SWE-bench_Verified` dataset from HuggingFace. Parses git patches to extract the "gold standard" files that were actually modified.

```python
from core.dataset import load_swebench, parse_gold_files

dataset = load_swebench()
task = dataset.get_task("scikit-learn__scikit-learn-10297")
print(task.gold_files)  # Files that need modification
```

### Environment (`core/env.py`)

Docker environment wrapper that:
- Creates isolated containers for each task
- Clones repositories at specific commits
- Mounts the gabb binary
- Initializes the gabb index

```python
from core.env import BenchmarkEnv, EnvConfig

config = EnvConfig(gabb_binary_path=Path("bin/gabb"))
async with BenchmarkEnv(config) as env:
    await env.setup(task)
    result = await env.exec("gabb symbols --name MyClass")
```

### Agents (`core/agent.py`)

Two agent implementations:

1. **ControlAgent**: Uses `grep`, `find_file`, `read_file`, `bash`
2. **GabbAgent**: Uses `gabb_symbols`, `gabb_definition`, `gabb_structure`, `gabb_usages`, `read_file`

Both agents follow the same system prompt and output `FINAL_ANSWER: <filepath>` when done.

### Tools (`core/tools.py`)

Tool definitions that wrap Docker command execution:

| Tool | Agent | Description |
|------|-------|-------------|
| `grep` | Control | Search for patterns in files |
| `find_file` | Control | Find files by name pattern |
| `read_file` | Both | Read file contents |
| `bash` | Control | Run arbitrary commands |
| `gabb_symbols` | Gabb | Search code symbols by name/pattern |
| `gabb_definition` | Gabb | Jump to symbol definition |
| `gabb_structure` | Gabb | Get file symbol outline |
| `gabb_usages` | Gabb | Find symbol references |

## Metrics

The benchmark collects:

| Metric | Description |
|--------|-------------|
| `tokens_input` | Input tokens consumed |
| `tokens_output` | Output tokens generated |
| `turns` | Number of conversation turns |
| `tool_calls` | Total tool invocations |
| `time_seconds` | Wall-clock time |
| `success` | Did the agent find the correct file? |

Derived metrics:
- **Token Savings**: `(control_tokens - gabb_tokens) / control_tokens * 100`
- **Speedup**: `control_time / gabb_time`

## Output

Results are saved to `results/`:

- `results_YYYYMMDD_HHMMSS.csv`: Spreadsheet-friendly format
- `results_YYYYMMDD_HHMMSS.json`: Full structured data

## Configuration

Create a `.env` file in the benchmark folder (use `.env.example` as a template):

```
ANTHROPIC_API_KEY=your-api-key-here
```

| Variable | Required | Description |
|----------|----------|-------------|
| `ANTHROPIC_API_KEY` | Yes | Anthropic API key (set in .env file) |
| `DOCKER_HOST` | No | Docker socket path (optional, env var) |

### Command Line Options

```
--task INSTANCE_ID     Run a specific task (Phase 1)
--tasks N              Number of tasks to run (Phase 2)
--concurrent N         Parallel containers (default: 1)
--model MODEL          Anthropic model (default: claude-sonnet-4-20250514)
--max-turns N          Max turns per agent (default: 20)
--no-save              Don't save results
-v, --verbose          Enable debug logging
```

## Development

### Running Tests

```bash
# Install dev dependencies
pip install -e ".[dev]"

# Run tests
pytest
```

### Adding New Tools

1. Create a new class extending `BaseTool` in `core/tools.py`
2. Implement `get_schema()` and `execute()`
3. Add to `get_control_tools()` or `get_gabb_tools()`

### Adding New Agents

1. Create a new class extending `BaseAgent` in `core/agent.py`
2. Implement `get_tools()` and `get_system_prompt()`
3. Register in `create_agent()` factory

## Phase Roadmap

### Phase 1: Walking Skeleton (Current)
- Single task end-to-end
- Control vs Gabb comparison
- Basic metrics collection

### Phase 2: Retrieval Suite
- Concurrent task execution
- Statistical analysis
- CSV/JSON reporting

### Phase 3: Full SWE-bench (Future)
- Full SWE-bench Docker images
- Code modification evaluation
- Test execution validation
