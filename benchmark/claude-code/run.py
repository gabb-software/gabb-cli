#!/usr/bin/env python3
"""
Claude Code Benchmark Runner.

Compares Claude Code performance with and without gabb MCP server + SKILL.md.

This benchmark measures:
- Whether SKILL.md guidance causes Claude Code to prefer gabb tools over Grep/Read
- Token usage differences between conditions
- Time to complete code navigation tasks

Supports both manual tasks (tasks.json) and SWE-bench tasks.

Usage:
    # Run a manual task with explicit workspace
    python run.py --task sklearn-ridge-normalize --workspace /path/to/sklearn

    # Run a SWE-bench task (auto-clones repo)
    python run.py --swe-bench scikit-learn__scikit-learn-10297

    # Run multiple SWE-bench tasks
    python run.py --swe-bench-suite --limit 10

    # List available tasks
    python run.py --list-tasks
    python run.py --list-swe-bench --repo scikit-learn
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Iterator

BENCHMARK_DIR = Path(__file__).parent
CONFIGS_DIR = BENCHMARK_DIR / "configs"
HOOKS_DIR = BENCHMARK_DIR / "hooks"
TASKS_FILE = BENCHMARK_DIR / "tasks" / "tasks.json"
RESULTS_DIR = BENCHMARK_DIR / "results"
API_ENV_FILE = BENCHMARK_DIR.parent / "api" / ".env"

# Add parent to path for shared modules
sys.path.insert(0, str(BENCHMARK_DIR.parent / "api"))


def load_env_file() -> dict[str, str]:
    """Load environment variables from api/.env file."""
    env_vars = {}
    if API_ENV_FILE.exists():
        for line in API_ENV_FILE.read_text().splitlines():
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                key, _, value = line.partition("=")
                env_vars[key.strip()] = value.strip()
    return env_vars

logger = logging.getLogger(__name__)

# Try to import rich for pretty output
try:
    from rich.console import Console
    from rich.table import Table
    from rich.progress import Progress, SpinnerColumn, TextColumn, BarColumn
    console = Console()
    HAS_RICH = True
except ImportError:
    HAS_RICH = False
    console = None


def print_msg(msg: str, style: str = "") -> None:
    """Print a message, using rich if available."""
    if HAS_RICH and console:
        console.print(f"[{style}]{msg}[/{style}]" if style else msg)
    else:
        print(msg)


# =============================================================================
# Data Models
# =============================================================================


@dataclass
class RunMetrics:
    """Metrics from a single Claude Code run."""

    task_id: str
    condition: str  # "control" or "gabb"

    # Timing
    wall_time_seconds: float = 0.0

    # Tokens
    tokens_input: int = 0
    tokens_output: int = 0

    # Tool usage counts
    tool_calls: dict[str, int] = field(default_factory=dict)

    # Outcome
    success: bool = False
    final_answer: str | None = None
    error: str | None = None

    # Conversation turns
    turns: int = 0

    def to_dict(self) -> dict[str, Any]:
        """Convert to dictionary for JSON export."""
        return {
            "task_id": self.task_id,
            "condition": self.condition,
            "wall_time_seconds": round(self.wall_time_seconds, 2),
            "tokens_input": self.tokens_input,
            "tokens_output": self.tokens_output,
            "tokens_total": self.tokens_input + self.tokens_output,
            "tool_calls": self.tool_calls,
            "tool_calls_total": sum(self.tool_calls.values()),
            "success": self.success,
            "final_answer": self.final_answer,
            "error": self.error,
            "turns": self.turns,
        }


@dataclass
class AggregateStats:
    """Aggregate statistics for multiple runs."""

    mean: float
    std: float
    min: float
    max: float

    def to_dict(self) -> dict[str, float]:
        return {
            "mean": round(self.mean, 2),
            "std": round(self.std, 2),
            "min": round(self.min, 2),
            "max": round(self.max, 2),
        }


def compute_stats(values: list[float]) -> AggregateStats:
    """Compute aggregate statistics for a list of values."""
    import statistics

    if not values:
        return AggregateStats(0, 0, 0, 0)
    if len(values) == 1:
        return AggregateStats(values[0], 0, values[0], values[0])

    return AggregateStats(
        mean=statistics.mean(values),
        std=statistics.stdev(values),
        min=min(values),
        max=max(values),
    )


def aggregate_runs(runs: list[RunMetrics]) -> dict[str, Any]:
    """Aggregate multiple runs into summary statistics."""
    if not runs:
        return {}

    # Collect values for each metric
    times = [r.wall_time_seconds for r in runs]
    tokens_input = [r.tokens_input for r in runs]
    tokens_output = [r.tokens_output for r in runs]
    tokens_total = [r.tokens_input + r.tokens_output for r in runs]
    tool_calls_total = [sum(r.tool_calls.values()) for r in runs]
    turns = [r.turns for r in runs]
    successes = sum(1 for r in runs if r.success)

    # Aggregate tool calls across all runs
    all_tools: dict[str, list[int]] = {}
    for r in runs:
        for tool, count in r.tool_calls.items():
            if tool not in all_tools:
                all_tools[tool] = []
            all_tools[tool].append(count)

    # For tools not used in some runs, add zeros
    for tool in all_tools:
        while len(all_tools[tool]) < len(runs):
            all_tools[tool].append(0)

    tool_stats = {tool: compute_stats(counts).to_dict() for tool, counts in all_tools.items()}

    return {
        "wall_time_seconds": compute_stats(times).to_dict(),
        "tokens_input": compute_stats(tokens_input).to_dict(),
        "tokens_output": compute_stats(tokens_output).to_dict(),
        "tokens_total": compute_stats(tokens_total).to_dict(),
        "tool_calls_total": compute_stats(tool_calls_total).to_dict(),
        "turns": compute_stats(turns).to_dict(),
        "success_rate": round(successes / len(runs), 2) if runs else 0,
        "success_count": successes,
        "run_count": len(runs),
        "tool_calls": tool_stats,
    }


@dataclass
class Task:
    """A benchmark task definition."""

    id: str
    repo: str
    prompt: str
    expected_files: list[str]
    base_commit: str = ""  # For SWE-bench tasks
    version: str = ""
    tags: list[str] = field(default_factory=list)

    @property
    def is_swe_bench(self) -> bool:
        """Check if this is a SWE-bench task."""
        return bool(self.base_commit)


# =============================================================================
# Task Loading
# =============================================================================


def load_manual_tasks() -> list[Task]:
    """Load task definitions from local JSON file."""
    if not TASKS_FILE.exists():
        return []
    with open(TASKS_FILE) as f:
        data = json.load(f)
    return [Task(**t) for t in data["tasks"]]


def get_manual_task(task_id: str) -> Task | None:
    """Get a specific manual task by ID."""
    for task in load_manual_tasks():
        if task.id == task_id:
            return task
    return None


def load_swe_bench() -> Any:
    """Load the SWE-bench dataset."""
    try:
        from core.dataset import load_swebench
        return load_swebench()
    except ImportError:
        print_msg("Error: Could not import SWE-bench dataset loader.", "red")
        print_msg("Make sure you're in the benchmark directory and dependencies are installed.", "dim")
        print_msg("Run: pip install datasets", "dim")
        return None


def swe_bench_task_to_task(swe_task: Any) -> Task:
    """Convert a SWE-bench task to our Task format."""
    # Build a prompt from the problem statement
    prompt = f"""Find the file(s) that need to be modified to fix this issue:

## Issue Description

{swe_task.problem_statement}

## Hints

{swe_task.hints_text if swe_task.hints_text else 'No hints available.'}

## Task

Identify the file(s) that contain the code that needs to be changed to address this issue."""

    return Task(
        id=swe_task.instance_id,
        repo=swe_task.repo,
        prompt=prompt,
        expected_files=swe_task.gold_files,
        base_commit=swe_task.base_commit,
        tags=["swe-bench"],
    )


# =============================================================================
# Workspace Management
# =============================================================================


class WorkspaceManager:
    """Manages repository workspaces for benchmarking."""

    DEFAULT_CACHE_DIR = Path.home() / ".cache" / "gabb-benchmark" / "repos"

    def __init__(self, cache_dir: Path | None = None):
        self.cache_dir = cache_dir or self.DEFAULT_CACHE_DIR
        self.cache_dir.mkdir(parents=True, exist_ok=True)

    def get_workspace(self, repo: str, commit: str) -> Path:
        """Get a workspace directory for a repo at a specific commit."""
        workspace_dir = self._get_commit_workspace(repo, commit)

        if not workspace_dir.exists():
            repo_dir = self._ensure_repo_cloned(repo)
            self._create_commit_workspace(repo_dir, workspace_dir, commit)

        return workspace_dir

    def _get_repo_cache_dir(self, repo: str) -> Path:
        safe_name = repo.replace("/", "__")
        return self.cache_dir / safe_name

    def _get_commit_workspace(self, repo: str, commit: str) -> Path:
        safe_name = repo.replace("/", "__")
        return self.cache_dir / "workspaces" / f"{safe_name}__{commit[:12]}"

    def _ensure_repo_cloned(self, repo: str) -> Path:
        repo_dir = self._get_repo_cache_dir(repo)
        if repo_dir.exists():
            return repo_dir

        url = f"https://github.com/{repo}.git"
        print_msg(f"Cloning {repo}...", "dim")

        # Try shallow clone first
        result = subprocess.run(
            ["git", "clone", "--depth", "1", "--no-single-branch", url, str(repo_dir)],
            capture_output=True,
            text=True,
        )

        if result.returncode != 0:
            # Fall back to full clone
            result = subprocess.run(
                ["git", "clone", url, str(repo_dir)],
                capture_output=True,
                text=True,
            )
            if result.returncode != 0:
                raise RuntimeError(f"Failed to clone {repo}: {result.stderr}")

        return repo_dir

    def _create_commit_workspace(self, repo_dir: Path, workspace_dir: Path, commit: str) -> None:
        print_msg(f"Creating workspace at {commit[:12]}...", "dim")

        workspace_dir.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(repo_dir, workspace_dir)

        # Fetch and checkout
        subprocess.run(
            ["git", "fetch", "origin", commit],
            cwd=workspace_dir,
            capture_output=True,
        )

        result = subprocess.run(
            ["git", "checkout", commit],
            cwd=workspace_dir,
            capture_output=True,
            text=True,
        )

        if result.returncode != 0:
            # Try unshallow and retry
            subprocess.run(
                ["git", "fetch", "--unshallow"],
                cwd=workspace_dir,
                capture_output=True,
            )
            result = subprocess.run(
                ["git", "checkout", commit],
                cwd=workspace_dir,
                capture_output=True,
                text=True,
            )
            if result.returncode != 0:
                raise RuntimeError(f"Failed to checkout {commit}: {result.stderr}")

    def cleanup_workspace(self, workspace_dir: Path) -> None:
        """Remove gabb artifacts from workspace."""
        gabb_dir = workspace_dir / ".gabb"
        if gabb_dir.exists():
            shutil.rmtree(gabb_dir)


# =============================================================================
# Claude Code Runner
# =============================================================================


class ClaudeCodeRunner:
    """Runs Claude Code with specific configuration."""

    def __init__(
        self,
        workspace: Path,
        condition: str,
        gabb_binary: Path | None = None,
        verbose: bool = False,
    ):
        self.workspace = workspace
        self.condition = condition
        self.gabb_binary = gabb_binary or shutil.which("gabb")
        self.verbose = verbose
        self.tool_log: Path | None = None
        self.temp_dir: Path | None = None
        self.workspace_claude_dir: Path | None = None
        self.mcp_config_file: Path | None = None
        self.skill_content: str | None = None  # Skill content to inject via system prompt
        # Track original CLAUDE.md state for cleanup
        self.original_claudemd: str | None = None  # Original content or None if didn't exist
        self.claudemd_existed: bool = False

    def setup(self) -> None:
        """Set up workspace-local config for Claude Code.

        Uses workspace-local .claude/ directory instead of CLAUDE_CONFIG_DIR
        to preserve authentication credentials stored in system keychain.
        """
        self.temp_dir = Path(tempfile.mkdtemp(prefix="claude_bench_"))
        self.tool_log = self.temp_dir / "tool_calls.jsonl"

        # Track original CLAUDE.md state for cleanup
        claudemd_path = self.workspace / "CLAUDE.md"
        self.claudemd_existed = claudemd_path.exists()
        if self.claudemd_existed:
            self.original_claudemd = claudemd_path.read_text()
        else:
            self.original_claudemd = None

        # For conditions that shouldn't have gabb CLAUDE.md guidance, ensure it's clean
        # This prevents interference from previous runs
        gabb_marker = "## Tool Selection: Use gabb"
        if self.condition in ("control", "gabb", "gabb-prompt") and self.claudemd_existed:
            if gabb_marker in self.original_claudemd:
                # Remove the gabb section from CLAUDE.md
                lines = self.original_claudemd.split("\n")
                new_lines = []
                skip_until_next_h2 = False
                for line in lines:
                    if line.startswith(gabb_marker):
                        skip_until_next_h2 = True
                        continue
                    if skip_until_next_h2 and line.startswith("## "):
                        skip_until_next_h2 = False
                    if not skip_until_next_h2:
                        new_lines.append(line)
                cleaned = "\n".join(new_lines).rstrip() + "\n"
                claudemd_path.write_text(cleaned)
                if self.verbose:
                    print_msg("  Removed gabb section from CLAUDE.md for clean run", "dim")

        # Use workspace-local .claude directory for project-specific settings
        self.workspace_claude_dir = self.workspace / ".claude"
        self.workspace_claude_dir.mkdir(parents=True, exist_ok=True)

        # Build settings for this run
        settings: dict[str, Any] = {}

        # Add tool tracking hook
        hook_script = HOOKS_DIR / "tool_tracker.py"
        settings["hooks"] = {
            "PostToolUse": [
                {
                    "matcher": ".*",
                    "hooks": [
                        {
                            "type": "command",
                            "command": f"python3 {hook_script}",
                        }
                    ],
                }
            ]
        }

        # Configure gabb MCP server for gabb/gabb-prompt/gabb-claudemd conditions
        if self.condition in ("gabb", "gabb-prompt", "gabb-claudemd") and self.gabb_binary:
            settings["mcpServers"] = {
                "gabb": {
                    "command": str(self.gabb_binary),
                    "args": ["mcp-server", "--workspace", str(self.workspace)],
                }
            }
            # Also create a separate MCP config file for --mcp-config flag
            self.mcp_config_file = self.temp_dir / "mcp_config.json"
            self.mcp_config_file.write_text(json.dumps({
                "mcpServers": settings["mcpServers"]
            }, indent=2))

        # Write workspace-local settings
        (self.workspace_claude_dir / "settings.local.json").write_text(
            json.dumps(settings, indent=2)
        )

        # Load skill content for gabb/gabb-prompt conditions (NOT gabb-claudemd)
        # NOTE: Skills don't work in -p (print) mode, so we inject via --append-system-prompt
        # gabb-claudemd uses CLAUDE.md instead of system prompt injection
        if self.condition in ("gabb", "gabb-prompt"):
            skill_file = CONFIGS_DIR / "gabb" / "skills" / "gabb" / "SKILL.md"
            if skill_file.exists():
                # Read skill content, stripping YAML frontmatter
                content = skill_file.read_text()
                # Remove frontmatter (everything between --- markers)
                if content.startswith("---"):
                    end_marker = content.find("---", 3)
                    if end_marker != -1:
                        content = content[end_marker + 3:].strip()
                self.skill_content = content
                if self.verbose:
                    print_msg(f"Loaded skill content ({len(self.skill_content)} chars)", "dim")

        # Initialize gabb for gabb/gabb-prompt/gabb-claudemd conditions
        if self.condition in ("gabb", "gabb-prompt", "gabb-claudemd") and self.gabb_binary:
            self._setup_gabb()

        # For gabb-claudemd, add gabb guidance to CLAUDE.md
        if self.condition == "gabb-claudemd" and self.gabb_binary:
            self._setup_claudemd()

    def _setup_gabb(self) -> None:
        """Initialize gabb index in workspace."""
        if self.verbose:
            print_msg(f"Initializing gabb in {self.workspace}...", "dim")

        subprocess.run(
            [str(self.gabb_binary), "init"],
            cwd=self.workspace,
            capture_output=True,
        )

        # Start daemon in background mode
        result = subprocess.run(
            [str(self.gabb_binary), "daemon", "start", "-b"],
            cwd=self.workspace,
            capture_output=True,
            text=True,
        )

        if result.returncode != 0:
            if self.verbose:
                print_msg(f"gabb daemon start warning: {result.stderr[:200]}", "yellow")

        # Wait for daemon to be ready with an index
        # Poll status until indexed file count > 0 or timeout
        max_wait = 300  # 5 minutes for large repos
        poll_interval = 2
        waited = 0

        while waited < max_wait:
            status_result = subprocess.run(
                [str(self.gabb_binary), "daemon", "status", "--format", "json"],
                cwd=self.workspace,
                capture_output=True,
                text=True,
            )

            if status_result.returncode == 0:
                try:
                    status = json.loads(status_result.stdout)
                    # Check if daemon is running and has indexed files
                    # Stats are nested under "stats" key
                    stats = status.get("stats", {})
                    files_indexed = stats.get("files_indexed", 0)
                    if status.get("running") and files_indexed > 0:
                        if self.verbose:
                            print_msg(
                                f"gabb ready: {files_indexed} files indexed",
                                "green"
                            )
                        return
                except json.JSONDecodeError:
                    pass

            time.sleep(poll_interval)
            waited += poll_interval

            if self.verbose and waited % 10 == 0:
                print_msg(f"  waiting for gabb indexing... ({waited}s)", "dim")

        if self.verbose:
            print_msg(f"gabb warning: timeout waiting for index after {max_wait}s", "yellow")

    def _setup_claudemd(self) -> None:
        """Add gabb guidance to CLAUDE.md using gabb init --claudemd."""
        if self.verbose:
            print_msg("  Adding gabb guidance to CLAUDE.md...", "dim")

        result = subprocess.run(
            [str(self.gabb_binary), "init", "--claudemd"],
            cwd=self.workspace,
            capture_output=True,
            text=True,
        )

        if result.returncode != 0:
            if self.verbose:
                print_msg(f"  gabb init --claudemd warning: {result.stderr[:200]}", "yellow")
        elif self.verbose:
            print_msg("  CLAUDE.md configured with gabb guidance", "green")

    def run(self, prompt: str, timeout: int = 300) -> RunMetrics:
        """Run Claude Code with the given prompt."""
        metrics = RunMetrics(task_id="", condition=self.condition)

        if not self.tool_log:
            metrics.error = "Runner not set up"
            return metrics

        if self.tool_log and self.tool_log.exists():
            self.tool_log.unlink()

        # Build environment with API key from .env file
        env = os.environ.copy()
        env.update(load_env_file())  # Load ANTHROPIC_API_KEY from api/.env
        env["BENCHMARK_TOOL_LOG"] = str(self.tool_log)

        # Add gabb usage hint for gabb-prompt condition
        if self.condition == "gabb-prompt":
            gabb_hint = """IMPORTANT: This project has gabb MCP tools available. Use gabb_symbols and gabb_structure instead of Grep/Read for code navigation.

"""
        else:
            gabb_hint = ""

        full_prompt = f"""{gabb_hint}{prompt}

When you find the file(s), output your answer in this format:
FINAL_ANSWER: path/to/file.py

If multiple files, list each on a new line:
FINAL_ANSWER: path/to/file1.py
FINAL_ANSWER: path/to/file2.py"""

        cmd = ["claude", "-p", full_prompt, "--output-format", "json"]

        # Add MCP config for gabb/gabb-prompt conditions
        if self.mcp_config_file and self.mcp_config_file.exists():
            cmd.extend(["--mcp-config", str(self.mcp_config_file)])
            # Allow all gabb MCP tools
            cmd.extend([
                "--allowedTools",
                "mcp__gabb__gabb_symbols",
                "mcp__gabb__gabb_symbol",
                "mcp__gabb__gabb_definition",
                "mcp__gabb__gabb_usages",
                "mcp__gabb__gabb_implementations",
                "mcp__gabb__gabb_daemon_status",
                "mcp__gabb__gabb_duplicates",
                "mcp__gabb__gabb_includers",
                "mcp__gabb__gabb_includes",
                "mcp__gabb__gabb_structure",
                "mcp__gabb__gabb_supertypes",
                "mcp__gabb__gabb_subtypes",
                "mcp__gabb__gabb_rename",
                "mcp__gabb__gabb_callers",
                "mcp__gabb__gabb_callees",
                "mcp__gabb__gabb_stats",
            ])

        # Inject skill content via system prompt (skills don't work in -p mode)
        if self.skill_content:
            cmd.extend(["--append-system-prompt", self.skill_content])

        if self.verbose:
            print_msg(f"Running claude in {self.workspace}", "dim")

        start_time = time.time()
        try:
            result = subprocess.run(
                cmd,
                cwd=self.workspace,
                env=env,
                capture_output=True,
                text=True,
                timeout=timeout,
            )
            metrics.wall_time_seconds = time.time() - start_time

            if result.returncode == 0:
                try:
                    output = json.loads(result.stdout)
                    metrics.final_answer = output.get("result", "")
                    # Tokens are nested under 'usage' in Claude Code output
                    usage = output.get("usage", {})
                    metrics.tokens_input = usage.get("input_tokens", 0)
                    metrics.tokens_output = usage.get("output_tokens", 0)
                    # Also capture cache tokens if available
                    metrics.tokens_input += usage.get("cache_read_input_tokens", 0)
                    metrics.tokens_input += usage.get("cache_creation_input_tokens", 0)
                    metrics.turns = output.get("num_turns", 0)
                except json.JSONDecodeError:
                    metrics.final_answer = result.stdout
            else:
                metrics.error = result.stderr or f"Exit code: {result.returncode}"

        except subprocess.TimeoutExpired:
            metrics.wall_time_seconds = timeout
            metrics.error = f"Timeout after {timeout}s"
        except FileNotFoundError:
            metrics.error = "Claude Code CLI not found"
        except Exception as e:
            metrics.error = str(e)

        # Parse tool log
        if self.tool_log and self.tool_log.exists():
            for line in self.tool_log.read_text().splitlines():
                try:
                    record = json.loads(line)
                    tool = record.get("tool_name", "unknown")
                    metrics.tool_calls[tool] = metrics.tool_calls.get(tool, 0) + 1
                except json.JSONDecodeError:
                    pass

        return metrics

    def cleanup(self) -> None:
        """Clean up temporary resources."""
        if self.condition in ("gabb", "gabb-prompt", "gabb-claudemd") and self.gabb_binary:
            subprocess.run(
                [str(self.gabb_binary), "daemon", "stop"],
                cwd=self.workspace,
                capture_output=True,
            )

        if self.temp_dir and self.temp_dir.exists():
            shutil.rmtree(self.temp_dir, ignore_errors=True)

        # Restore original CLAUDE.md state
        claudemd_path = self.workspace / "CLAUDE.md"
        if self.claudemd_existed:
            # Restore original content
            if self.original_claudemd is not None:
                claudemd_path.write_text(self.original_claudemd)
        else:
            # CLAUDE.md didn't exist before, remove if we created it
            if claudemd_path.exists():
                claudemd_path.unlink()

        # Clean up workspace-local settings we created
        if self.workspace_claude_dir and self.workspace_claude_dir.exists():
            settings_file = self.workspace_claude_dir / "settings.local.json"
            if settings_file.exists():
                settings_file.unlink()
            skills_dir = self.workspace_claude_dir / "skills" / "gabb"
            if skills_dir.exists():
                shutil.rmtree(skills_dir, ignore_errors=True)
            # Clean up skills directory if empty
            skills_parent = self.workspace_claude_dir / "skills"
            if skills_parent.exists() and not list(skills_parent.iterdir()):
                skills_parent.rmdir()
            # Clean up .claude directory if empty (but not if it has other content)
            if self.workspace_claude_dir.exists() and not list(self.workspace_claude_dir.iterdir()):
                self.workspace_claude_dir.rmdir()


# =============================================================================
# Benchmark Execution
# =============================================================================


def check_success(answer: str | None, expected_files: list[str]) -> bool:
    """Check if the answer contains expected files."""
    if not answer:
        return False
    answer_lower = answer.lower()
    for f in expected_files:
        if f.lower() in answer_lower or Path(f).name.lower() in answer_lower:
            return True
    return False


def run_single_condition(
    task: Task,
    workspace: Path,
    condition: str,
    gabb_binary: Path | None = None,
    verbose: bool = False,
    run_number: int | None = None,
    total_runs: int | None = None,
) -> RunMetrics:
    """Run a single condition and return metrics."""
    if run_number is not None and total_runs is not None:
        print_msg(f"  [{run_number}/{total_runs}] {condition}...", "cyan")
    else:
        print_msg(f"  Running {condition}...", "cyan")

    runner = ClaudeCodeRunner(
        workspace=workspace,
        condition=condition,
        gabb_binary=gabb_binary,
        verbose=verbose,
    )

    try:
        runner.setup()
        metrics = runner.run(task.prompt)
        metrics.task_id = task.id
        metrics.success = check_success(metrics.final_answer, task.expected_files)

        # Print brief result for multi-run mode
        if run_number is not None:
            tokens = metrics.tokens_input + metrics.tokens_output
            status = "✓" if metrics.success else "✗"
            print_msg(
                f"      → {metrics.wall_time_seconds:.1f}s, {tokens:,} tokens, {status}",
                "green" if metrics.success else "red"
            )

        return metrics
    finally:
        runner.cleanup()


def run_multiple(
    task: Task,
    workspace: Path,
    condition: str,
    run_count: int,
    gabb_binary: Path | None = None,
    verbose: bool = False,
) -> list[RunMetrics]:
    """Run a condition multiple times and return all results."""
    results = []
    for i in range(run_count):
        metrics = run_single_condition(
            task, workspace, condition, gabb_binary, verbose,
            run_number=i + 1, total_runs=run_count
        )
        results.append(metrics)
    return results


def run_comparison(
    task: Task,
    workspace: Path,
    gabb_binary: Path | None = None,
    verbose: bool = False,
    run_count: int = 1,
) -> tuple[list[RunMetrics], list[RunMetrics]]:
    """Run both conditions on a task, optionally multiple times."""
    control_runs = run_multiple(task, workspace, "control", run_count, gabb_binary, verbose)
    gabb_runs = run_multiple(task, workspace, "gabb", run_count, gabb_binary, verbose)
    return control_runs, gabb_runs


def run_all_conditions(
    task: Task,
    workspace: Path,
    gabb_binary: Path | None = None,
    verbose: bool = False,
    run_count: int = 1,
) -> dict[str, list[RunMetrics]]:
    """Run all three conditions (control, gabb, gabb-prompt) on a task."""
    return {
        "control": run_multiple(task, workspace, "control", run_count, gabb_binary, verbose),
        "gabb": run_multiple(task, workspace, "gabb", run_count, gabb_binary, verbose),
        "gabb-prompt": run_multiple(task, workspace, "gabb-prompt", run_count, gabb_binary, verbose),
    }


def run_gabb_conditions(
    task: Task,
    workspace: Path,
    gabb_binary: Path | None = None,
    verbose: bool = False,
    run_count: int = 1,
) -> dict[str, list[RunMetrics]]:
    """Run gabb and gabb-prompt conditions (skip control)."""
    return {
        "gabb": run_multiple(task, workspace, "gabb", run_count, gabb_binary, verbose),
        "gabb-prompt": run_multiple(task, workspace, "gabb-prompt", run_count, gabb_binary, verbose),
    }


def run_full_conditions(
    task: Task,
    workspace: Path,
    gabb_binary: Path | None = None,
    verbose: bool = False,
    run_count: int = 1,
) -> dict[str, list[RunMetrics]]:
    """Run all four conditions (control, gabb, gabb-prompt, gabb-claudemd) on a task."""
    return {
        "control": run_multiple(task, workspace, "control", run_count, gabb_binary, verbose),
        "gabb": run_multiple(task, workspace, "gabb", run_count, gabb_binary, verbose),
        "gabb-prompt": run_multiple(task, workspace, "gabb-prompt", run_count, gabb_binary, verbose),
        "gabb-claudemd": run_multiple(task, workspace, "gabb-claudemd", run_count, gabb_binary, verbose),
    }


# =============================================================================
# Output / Reporting
# =============================================================================


def print_comparison(
    control: list[RunMetrics] | RunMetrics,
    gabb: list[RunMetrics] | RunMetrics,
) -> None:
    """Print comparison of runs (single or multiple)."""
    # Normalize to lists
    control_runs = control if isinstance(control, list) else [control]
    gabb_runs = gabb if isinstance(gabb, list) else [gabb]

    if HAS_RICH and console:
        _print_comparison_rich(control_runs, gabb_runs)
    else:
        _print_comparison_plain(control_runs, gabb_runs)


def print_all_conditions(results: dict[str, list[RunMetrics]]) -> None:
    """Print comparison of all three conditions."""
    if HAS_RICH and console:
        _print_all_conditions_rich(results)
    else:
        _print_all_conditions_plain(results)


def print_gabb_conditions(results: dict[str, list[RunMetrics]]) -> None:
    """Print comparison of gabb vs gabb-prompt."""
    if HAS_RICH and console:
        _print_gabb_conditions_rich(results)
    else:
        _print_gabb_conditions_plain(results)


def _print_gabb_conditions_rich(results: dict[str, list[RunMetrics]]) -> None:
    """Print gabb vs gabb-prompt comparison using rich."""
    gabb_agg = aggregate_runs(results.get("gabb", []))
    prompt_agg = aggregate_runs(results.get("gabb-prompt", []))

    any_runs = results.get("gabb", results.get("gabb-prompt", []))
    single_run = len(any_runs) == 1 if any_runs else True
    task_id = any_runs[0].task_id if any_runs else "Unknown"

    title = f"Results: {task_id} (gabb vs gabb-prompt)"
    if not single_run:
        title += f" - {len(any_runs)} runs"

    table = Table(title=title)
    table.add_column("Metric", style="cyan")
    table.add_column("Gabb", justify="right")
    table.add_column("Gabb+Prompt", justify="right")
    table.add_column("Diff", justify="right")

    def fmt(agg: dict, key: str) -> float:
        if not agg:
            return 0
        val = agg.get(key, {})
        return val.get("mean", 0) if isinstance(val, dict) else 0

    def fmt_str(agg: dict, key: str) -> str:
        if not agg:
            return "-"
        val = agg.get(key, {})
        if isinstance(val, dict):
            if single_run:
                return f"{val.get('mean', 0):.1f}"
            return f"{val.get('mean', 0):.1f} ± {val.get('std', 0):.1f}"
        return str(val)

    def fmt_success(agg: dict) -> str:
        if not agg:
            return "-"
        rate = agg.get("success_rate", 0)
        if single_run:
            return "[green]PASS[/green]" if rate == 1 else "[red]FAIL[/red]"
        return f"{rate * 100:.0f}%"

    # Success
    table.add_row("Success", fmt_success(gabb_agg), fmt_success(prompt_agg), "")

    # Time
    g_time = fmt(gabb_agg, "wall_time_seconds")
    p_time = fmt(prompt_agg, "wall_time_seconds")
    diff = p_time - g_time
    pct = (diff / g_time * 100) if g_time > 0 else 0
    table.add_row("Time (s)", fmt_str(gabb_agg, "wall_time_seconds"), fmt_str(prompt_agg, "wall_time_seconds"), f"{diff:+.1f} ({pct:+.0f}%)")

    # Tokens
    g_tokens = fmt(gabb_agg, "tokens_total")
    p_tokens = fmt(prompt_agg, "tokens_total")
    diff = p_tokens - g_tokens
    pct = (diff / g_tokens * 100) if g_tokens > 0 else 0
    table.add_row("Tokens", f"{g_tokens:,.0f}", f"{p_tokens:,.0f}", f"{diff:+,.0f} ({pct:+.0f}%)")

    # Tool calls
    table.add_row("Tool Calls", fmt_str(gabb_agg, "tool_calls_total"), fmt_str(prompt_agg, "tool_calls_total"), "")

    console.print(table)

    # Tool breakdown
    console.print("\n[bold]Tool Usage:[/bold]")
    all_tools: set[str] = set()
    for agg in [gabb_agg, prompt_agg]:
        if agg:
            all_tools.update(agg.get("tool_calls", {}).keys())

    gabb_tool_names = sorted([t for t in all_tools if "gabb" in t.lower()])
    search_tools = sorted([t for t in all_tools if t in ("Grep", "Glob", "Read")])
    other_tools = sorted([t for t in all_tools if t not in gabb_tool_names and t not in search_tools])

    tool_table = Table()
    tool_table.add_column("Tool", style="cyan")
    tool_table.add_column("Gabb", justify="right")
    tool_table.add_column("Gabb+Prompt", justify="right")

    def get_tool_stat(agg: dict, tool: str) -> str:
        if not agg:
            return "-"
        tools = agg.get("tool_calls", {})
        t = tools.get(tool, {"mean": 0})
        if single_run:
            return f"{t.get('mean', 0):.0f}"
        return f"{t.get('mean', 0):.1f}"

    for tool in search_tools + gabb_tool_names + other_tools:
        g = get_tool_stat(gabb_agg, tool)
        p = get_tool_stat(prompt_agg, tool)
        if g != "0" or p != "0":
            tool_table.add_row(tool, g, p)

    console.print(tool_table)


def _print_gabb_conditions_plain(results: dict[str, list[RunMetrics]]) -> None:
    """Print gabb vs gabb-prompt comparison in plain text."""
    gabb_agg = aggregate_runs(results.get("gabb", []))
    prompt_agg = aggregate_runs(results.get("gabb-prompt", []))

    any_runs = results.get("gabb", results.get("gabb-prompt", []))
    single_run = len(any_runs) == 1 if any_runs else True
    task_id = any_runs[0].task_id if any_runs else "Unknown"

    print(f"\n{'=' * 60}")
    title = f"Results: {task_id} (gabb vs gabb-prompt)"
    if not single_run:
        title += f" - {len(any_runs)} runs"
    print(title)
    print('=' * 60)
    print(f"{'Metric':<15} {'Gabb':>20} {'Gabb+Prompt':>20}")
    print('-' * 60)

    def fmt(agg: dict, key: str) -> str:
        if not agg:
            return "-"
        val = agg.get(key, {})
        if isinstance(val, dict):
            return f"{val.get('mean', 0):.1f}"
        return str(val)

    def fmt_success(agg: dict) -> str:
        if not agg:
            return "-"
        rate = agg.get("success_rate", 0)
        if single_run:
            return "PASS" if rate == 1 else "FAIL"
        return f"{rate * 100:.0f}%"

    print(f"{'Success':<15} {fmt_success(gabb_agg):>20} {fmt_success(prompt_agg):>20}")
    print(f"{'Time (s)':<15} {fmt(gabb_agg, 'wall_time_seconds'):>20} {fmt(prompt_agg, 'wall_time_seconds'):>20}")
    print(f"{'Tool Calls':<15} {fmt(gabb_agg, 'tool_calls_total'):>20} {fmt(prompt_agg, 'tool_calls_total'):>20}")


def print_full_conditions(results: dict[str, list[RunMetrics]]) -> None:
    """Print comparison of all four conditions."""
    if HAS_RICH and console:
        _print_full_conditions_rich(results)
    else:
        _print_full_conditions_plain(results)


def _print_full_conditions_rich(results: dict[str, list[RunMetrics]]) -> None:
    """Print all four conditions comparison using rich."""
    control_agg = aggregate_runs(results.get("control", []))
    gabb_agg = aggregate_runs(results.get("gabb", []))
    prompt_agg = aggregate_runs(results.get("gabb-prompt", []))
    claudemd_agg = aggregate_runs(results.get("gabb-claudemd", []))

    any_runs = results.get("control", results.get("gabb", results.get("gabb-prompt", results.get("gabb-claudemd", []))))
    single_run = len(any_runs) == 1 if any_runs else True
    task_id = any_runs[0].task_id if any_runs else "Unknown"

    title = f"Results: {task_id} (Full Comparison)"
    if not single_run:
        title += f" - {len(any_runs)} runs"

    table = Table(title=title)
    table.add_column("Metric", style="cyan")
    table.add_column("Control", justify="right")
    table.add_column("Gabb", justify="right")
    table.add_column("Gabb+Prompt", justify="right")
    table.add_column("Gabb+CLAUDE.md", justify="right")

    # Helper to format values
    def fmt(agg: dict, key: str) -> str:
        if not agg:
            return "-"
        val = agg.get(key, {})
        if isinstance(val, dict):
            if single_run:
                return f"{val.get('mean', 0):.1f}"
            return f"{val.get('mean', 0):.1f} ± {val.get('std', 0):.1f}"
        return str(val)

    def fmt_tokens(agg: dict) -> str:
        if not agg:
            return "-"
        val = agg.get("tokens_total", {})
        if single_run:
            return f"{val.get('mean', 0):,.0f}"
        return f"{val.get('mean', 0):,.0f}"

    def fmt_success(agg: dict) -> str:
        if not agg:
            return "-"
        rate = agg.get("success_rate", 0)
        if single_run:
            return "[green]PASS[/green]" if rate == 1 else "[red]FAIL[/red]"
        return f"{rate * 100:.0f}%"

    table.add_row("Success", fmt_success(control_agg), fmt_success(gabb_agg), fmt_success(prompt_agg), fmt_success(claudemd_agg))
    table.add_row("Time (s)", fmt(control_agg, "wall_time_seconds"), fmt(gabb_agg, "wall_time_seconds"), fmt(prompt_agg, "wall_time_seconds"), fmt(claudemd_agg, "wall_time_seconds"))
    table.add_row("Tokens", fmt_tokens(control_agg), fmt_tokens(gabb_agg), fmt_tokens(prompt_agg), fmt_tokens(claudemd_agg))
    table.add_row("Tool Calls", fmt(control_agg, "tool_calls_total"), fmt(gabb_agg, "tool_calls_total"), fmt(prompt_agg, "tool_calls_total"), fmt(claudemd_agg, "tool_calls_total"))

    console.print(table)

    # Tool breakdown
    console.print("\n[bold]Tool Usage:[/bold]")
    all_tools: set[str] = set()
    for agg in [control_agg, gabb_agg, prompt_agg, claudemd_agg]:
        if agg:
            all_tools.update(agg.get("tool_calls", {}).keys())

    gabb_tool_names = sorted([t for t in all_tools if "gabb" in t.lower()])
    search_tools = sorted([t for t in all_tools if t in ("Grep", "Glob", "Read")])
    other_tools = sorted([t for t in all_tools if t not in gabb_tool_names and t not in search_tools])

    tool_table = Table()
    tool_table.add_column("Tool", style="cyan")
    tool_table.add_column("Control", justify="right")
    tool_table.add_column("Gabb", justify="right")
    tool_table.add_column("Gabb+Prompt", justify="right")
    tool_table.add_column("Gabb+CLAUDE.md", justify="right")

    def get_tool_stat(agg: dict, tool: str) -> str:
        if not agg:
            return "-"
        tools = agg.get("tool_calls", {})
        t = tools.get(tool, {"mean": 0})
        if single_run:
            return f"{t.get('mean', 0):.0f}"
        return f"{t.get('mean', 0):.1f}"

    for tool in search_tools + gabb_tool_names + other_tools:
        c = get_tool_stat(control_agg, tool)
        g = get_tool_stat(gabb_agg, tool)
        p = get_tool_stat(prompt_agg, tool)
        m = get_tool_stat(claudemd_agg, tool)
        if c != "0" or g != "0" or p != "0" or m != "0":
            tool_table.add_row(tool, c, g, p, m)

    console.print(tool_table)


def _print_full_conditions_plain(results: dict[str, list[RunMetrics]]) -> None:
    """Print all four conditions comparison in plain text."""
    control_agg = aggregate_runs(results.get("control", []))
    gabb_agg = aggregate_runs(results.get("gabb", []))
    prompt_agg = aggregate_runs(results.get("gabb-prompt", []))
    claudemd_agg = aggregate_runs(results.get("gabb-claudemd", []))

    any_runs = results.get("control", results.get("gabb", results.get("gabb-prompt", results.get("gabb-claudemd", []))))
    single_run = len(any_runs) == 1 if any_runs else True
    task_id = any_runs[0].task_id if any_runs else "Unknown"

    print(f"\n{'=' * 90}")
    title = f"Results: {task_id} (Full Comparison)"
    if not single_run:
        title += f" - {len(any_runs)} runs"
    print(title)
    print('=' * 90)
    print(f"{'Metric':<15} {'Control':>15} {'Gabb':>15} {'Gabb+Prompt':>15} {'Gabb+CLAUDE.md':>18}")
    print('-' * 90)

    def fmt(agg: dict, key: str) -> str:
        if not agg:
            return "-"
        val = agg.get(key, {})
        if isinstance(val, dict):
            return f"{val.get('mean', 0):.1f}"
        return str(val)

    def fmt_success(agg: dict) -> str:
        if not agg:
            return "-"
        rate = agg.get("success_rate", 0)
        if single_run:
            return "PASS" if rate == 1 else "FAIL"
        return f"{rate * 100:.0f}%"

    print(f"{'Success':<15} {fmt_success(control_agg):>15} {fmt_success(gabb_agg):>15} {fmt_success(prompt_agg):>15} {fmt_success(claudemd_agg):>18}")
    print(f"{'Time (s)':<15} {fmt(control_agg, 'wall_time_seconds'):>15} {fmt(gabb_agg, 'wall_time_seconds'):>15} {fmt(prompt_agg, 'wall_time_seconds'):>15} {fmt(claudemd_agg, 'wall_time_seconds'):>18}")
    print(f"{'Tool Calls':<15} {fmt(control_agg, 'tool_calls_total'):>15} {fmt(gabb_agg, 'tool_calls_total'):>15} {fmt(prompt_agg, 'tool_calls_total'):>15} {fmt(claudemd_agg, 'tool_calls_total'):>18}")


def _print_all_conditions_rich(results: dict[str, list[RunMetrics]]) -> None:
    """Print all conditions comparison using rich."""
    control_agg = aggregate_runs(results.get("control", []))
    gabb_agg = aggregate_runs(results.get("gabb", []))
    prompt_agg = aggregate_runs(results.get("gabb-prompt", []))

    any_runs = results.get("control", results.get("gabb", results.get("gabb-prompt", [])))
    single_run = len(any_runs) == 1 if any_runs else True
    task_id = any_runs[0].task_id if any_runs else "Unknown"

    title = f"Results: {task_id}"
    if not single_run:
        title += f" ({len(any_runs)} runs)"

    table = Table(title=title)
    table.add_column("Metric", style="cyan")
    table.add_column("Control", justify="right")
    table.add_column("Gabb", justify="right")
    table.add_column("Gabb+Prompt", justify="right")

    # Helper to format values
    def fmt(agg: dict, key: str) -> str:
        if not agg:
            return "-"
        val = agg.get(key, {})
        if isinstance(val, dict):
            if single_run:
                return f"{val.get('mean', 0):.1f}"
            return f"{val.get('mean', 0):.1f} ± {val.get('std', 0):.1f}"
        return str(val)

    def fmt_tokens(agg: dict) -> str:
        if not agg:
            return "-"
        val = agg.get("tokens_total", {})
        if single_run:
            return f"{val.get('mean', 0):,.0f}"
        return f"{val.get('mean', 0):,.0f}"

    def fmt_success(agg: dict) -> str:
        if not agg:
            return "-"
        rate = agg.get("success_rate", 0)
        if single_run:
            return "[green]PASS[/green]" if rate == 1 else "[red]FAIL[/red]"
        return f"{rate * 100:.0f}%"

    table.add_row("Success", fmt_success(control_agg), fmt_success(gabb_agg), fmt_success(prompt_agg))
    table.add_row("Time (s)", fmt(control_agg, "wall_time_seconds"), fmt(gabb_agg, "wall_time_seconds"), fmt(prompt_agg, "wall_time_seconds"))
    table.add_row("Tokens", fmt_tokens(control_agg), fmt_tokens(gabb_agg), fmt_tokens(prompt_agg))
    table.add_row("Tool Calls", fmt(control_agg, "tool_calls_total"), fmt(gabb_agg, "tool_calls_total"), fmt(prompt_agg, "tool_calls_total"))

    console.print(table)

    # Tool breakdown
    console.print("\n[bold]Tool Usage:[/bold]")
    all_tools: set[str] = set()
    for agg in [control_agg, gabb_agg, prompt_agg]:
        if agg:
            all_tools.update(agg.get("tool_calls", {}).keys())

    gabb_tool_names = sorted([t for t in all_tools if "gabb" in t.lower()])
    search_tools = sorted([t for t in all_tools if t in ("Grep", "Glob", "Read")])
    other_tools = sorted([t for t in all_tools if t not in gabb_tool_names and t not in search_tools])

    tool_table = Table()
    tool_table.add_column("Tool", style="cyan")
    tool_table.add_column("Control", justify="right")
    tool_table.add_column("Gabb", justify="right")
    tool_table.add_column("Gabb+Prompt", justify="right")

    def get_tool_stat(agg: dict, tool: str) -> str:
        if not agg:
            return "-"
        tools = agg.get("tool_calls", {})
        t = tools.get(tool, {"mean": 0})
        if single_run:
            return f"{t.get('mean', 0):.0f}"
        return f"{t.get('mean', 0):.1f}"

    for tool in search_tools + gabb_tool_names + other_tools:
        c = get_tool_stat(control_agg, tool)
        g = get_tool_stat(gabb_agg, tool)
        p = get_tool_stat(prompt_agg, tool)
        if c != "0" or g != "0" or p != "0":
            tool_table.add_row(tool, c, g, p)

    console.print(tool_table)


def _print_all_conditions_plain(results: dict[str, list[RunMetrics]]) -> None:
    """Print all conditions comparison in plain text."""
    control_agg = aggregate_runs(results.get("control", []))
    gabb_agg = aggregate_runs(results.get("gabb", []))
    prompt_agg = aggregate_runs(results.get("gabb-prompt", []))

    any_runs = results.get("control", results.get("gabb", results.get("gabb-prompt", [])))
    single_run = len(any_runs) == 1 if any_runs else True
    task_id = any_runs[0].task_id if any_runs else "Unknown"

    print(f"\n{'=' * 70}")
    title = f"Results: {task_id}"
    if not single_run:
        title += f" ({len(any_runs)} runs)"
    print(title)
    print('=' * 70)
    print(f"{'Metric':<15} {'Control':>15} {'Gabb':>15} {'Gabb+Prompt':>15}")
    print('-' * 70)

    def fmt(agg: dict, key: str) -> str:
        if not agg:
            return "-"
        val = agg.get(key, {})
        if isinstance(val, dict):
            return f"{val.get('mean', 0):.1f}"
        return str(val)

    def fmt_success(agg: dict) -> str:
        if not agg:
            return "-"
        rate = agg.get("success_rate", 0)
        if single_run:
            return "PASS" if rate == 1 else "FAIL"
        return f"{rate * 100:.0f}%"

    print(f"{'Success':<15} {fmt_success(control_agg):>15} {fmt_success(gabb_agg):>15} {fmt_success(prompt_agg):>15}")
    print(f"{'Time (s)':<15} {fmt(control_agg, 'wall_time_seconds'):>15} {fmt(gabb_agg, 'wall_time_seconds'):>15} {fmt(prompt_agg, 'wall_time_seconds'):>15}")
    print(f"{'Tool Calls':<15} {fmt(control_agg, 'tool_calls_total'):>15} {fmt(gabb_agg, 'tool_calls_total'):>15} {fmt(prompt_agg, 'tool_calls_total'):>15}")


def _format_stat(stats: dict[str, float], single_run: bool = False) -> str:
    """Format a statistic for display."""
    if single_run:
        return f"{stats['mean']:.1f}"
    return f"{stats['mean']:.1f} ± {stats['std']:.1f}"


def _print_comparison_rich(control_runs: list[RunMetrics], gabb_runs: list[RunMetrics]) -> None:
    control_agg = aggregate_runs(control_runs)
    gabb_agg = aggregate_runs(gabb_runs)
    single_run = len(control_runs) == 1

    task_id = control_runs[0].task_id if control_runs else "Unknown"
    title = f"Results: {task_id}"
    if not single_run:
        title += f" ({len(control_runs)} runs)"

    table = Table(title=title)
    table.add_column("Metric", style="cyan")
    table.add_column("Control", justify="right")
    table.add_column("Gabb", justify="right")
    table.add_column("Diff", justify="right")

    # Success rate
    c_rate = control_agg["success_rate"]
    g_rate = gabb_agg["success_rate"]
    if single_run:
        c_success = "[green]PASS[/green]" if c_rate == 1 else "[red]FAIL[/red]"
        g_success = "[green]PASS[/green]" if g_rate == 1 else "[red]FAIL[/red]"
    else:
        c_success = f"{c_rate * 100:.0f}%"
        g_success = f"{g_rate * 100:.0f}%"
    table.add_row("Success", c_success, g_success, "")

    # Time
    c_time = control_agg["wall_time_seconds"]
    g_time = gabb_agg["wall_time_seconds"]
    time_diff = g_time["mean"] - c_time["mean"]
    time_pct = (time_diff / c_time["mean"] * 100) if c_time["mean"] > 0 else 0
    table.add_row(
        "Time (s)",
        _format_stat(c_time, single_run),
        _format_stat(g_time, single_run),
        f"{time_diff:+.1f} ({time_pct:+.0f}%)",
    )

    # Tokens
    c_tokens = control_agg["tokens_total"]
    g_tokens = gabb_agg["tokens_total"]
    token_diff = g_tokens["mean"] - c_tokens["mean"]
    token_pct = (token_diff / c_tokens["mean"] * 100) if c_tokens["mean"] > 0 else 0
    if single_run:
        table.add_row(
            "Total Tokens",
            f"{c_tokens['mean']:,.0f}",
            f"{g_tokens['mean']:,.0f}",
            f"{token_diff:+,.0f} ({token_pct:+.0f}%)",
        )
    else:
        table.add_row(
            "Total Tokens",
            f"{c_tokens['mean']:,.0f} ± {c_tokens['std']:,.0f}",
            f"{g_tokens['mean']:,.0f} ± {g_tokens['std']:,.0f}",
            f"{token_diff:+,.0f} ({token_pct:+.0f}%)",
        )

    # Tool calls
    c_calls = control_agg["tool_calls_total"]
    g_calls = gabb_agg["tool_calls_total"]
    call_diff = g_calls["mean"] - c_calls["mean"]
    table.add_row(
        "Tool Calls",
        _format_stat(c_calls, single_run),
        _format_stat(g_calls, single_run),
        f"{call_diff:+.1f}",
    )

    console.print(table)

    # Tool breakdown
    console.print("\n[bold]Tool Usage:[/bold]")
    c_tools = control_agg.get("tool_calls", {})
    g_tools = gabb_agg.get("tool_calls", {})
    all_tools = set(c_tools.keys()) | set(g_tools.keys())
    gabb_tool_names = sorted([t for t in all_tools if "gabb" in t.lower()])
    search_tools = sorted([t for t in all_tools if t in ("Grep", "Glob", "Read")])
    other_tools = sorted([t for t in all_tools if t not in gabb_tool_names and t not in search_tools])

    tool_table = Table()
    tool_table.add_column("Tool", style="cyan")
    tool_table.add_column("Control", justify="right")
    tool_table.add_column("Gabb", justify="right")

    for tool in search_tools + gabb_tool_names + other_tools:
        c = c_tools.get(tool, {"mean": 0, "std": 0})
        g = g_tools.get(tool, {"mean": 0, "std": 0})
        if c["mean"] > 0 or g["mean"] > 0:
            tool_table.add_row(
                tool,
                _format_stat(c, single_run),
                _format_stat(g, single_run),
            )

    console.print(tool_table)


def _print_comparison_plain(control_runs: list[RunMetrics], gabb_runs: list[RunMetrics]) -> None:
    control_agg = aggregate_runs(control_runs)
    gabb_agg = aggregate_runs(gabb_runs)
    single_run = len(control_runs) == 1

    task_id = control_runs[0].task_id if control_runs else "Unknown"
    print(f"\n{'=' * 60}")
    title = f"Results: {task_id}"
    if not single_run:
        title += f" ({len(control_runs)} runs)"
    print(title)
    print('=' * 60)
    print(f"{'Metric':<20} {'Control':>18} {'Gabb':>18}")
    print('-' * 60)

    # Success
    c_rate = control_agg["success_rate"]
    g_rate = gabb_agg["success_rate"]
    if single_run:
        c_status = "PASS" if c_rate == 1 else "FAIL"
        g_status = "PASS" if g_rate == 1 else "FAIL"
    else:
        c_status = f"{c_rate * 100:.0f}%"
        g_status = f"{g_rate * 100:.0f}%"
    print(f"{'Success':<20} {c_status:>18} {g_status:>18}")

    # Time
    c_time = control_agg["wall_time_seconds"]
    g_time = gabb_agg["wall_time_seconds"]
    print(f"{'Time (s)':<20} {_format_stat(c_time, single_run):>18} {_format_stat(g_time, single_run):>18}")

    # Tokens
    c_tokens = control_agg["tokens_total"]
    g_tokens = gabb_agg["tokens_total"]
    if single_run:
        print(f"{'Total Tokens':<20} {c_tokens['mean']:>18,.0f} {g_tokens['mean']:>18,.0f}")
    else:
        c_str = f"{c_tokens['mean']:,.0f} ± {c_tokens['std']:,.0f}"
        g_str = f"{g_tokens['mean']:,.0f} ± {g_tokens['std']:,.0f}"
        print(f"{'Total Tokens':<20} {c_str:>18} {g_str:>18}")

    # Tool calls
    c_calls = control_agg["tool_calls_total"]
    g_calls = gabb_agg["tool_calls_total"]
    print(f"{'Tool Calls':<20} {_format_stat(c_calls, single_run):>18} {_format_stat(g_calls, single_run):>18}")

    # Tool breakdown
    print("\nTool Usage:")
    c_tools = control_agg.get("tool_calls", {})
    g_tools = gabb_agg.get("tool_calls", {})
    for tool in sorted(set(c_tools.keys()) | set(g_tools.keys())):
        c = c_tools.get(tool, {"mean": 0, "std": 0})
        g = g_tools.get(tool, {"mean": 0, "std": 0})
        if c["mean"] > 0 or g["mean"] > 0:
            print(f"  {tool:<30} {_format_stat(c, single_run):>12} {_format_stat(g, single_run):>12}")


def print_single_condition(runs: list[RunMetrics], condition: str) -> None:
    """Print results for a single condition."""
    if HAS_RICH and console:
        _print_single_condition_rich(runs, condition)
    else:
        _print_single_condition_plain(runs, condition)


def _print_single_condition_rich(runs: list[RunMetrics], condition: str) -> None:
    """Print single condition results using rich."""
    agg = aggregate_runs(runs)
    single_run = len(runs) == 1

    task_id = runs[0].task_id if runs else "Unknown"
    title = f"Results: {task_id} ({condition})"
    if not single_run:
        title += f" - {len(runs)} runs"

    table = Table(title=title)
    table.add_column("Metric", style="cyan")
    table.add_column("Value", justify="right")

    # Success rate
    rate = agg["success_rate"]
    if single_run:
        success = "[green]PASS[/green]" if rate == 1 else "[red]FAIL[/red]"
    else:
        success = f"{rate * 100:.0f}%"
    table.add_row("Success", success)

    # Time
    time_stats = agg["wall_time_seconds"]
    table.add_row("Time (s)", _format_stat(time_stats, single_run))

    # Tokens
    tokens = agg["tokens_total"]
    if single_run:
        table.add_row("Total Tokens", f"{tokens['mean']:,.0f}")
    else:
        table.add_row("Total Tokens", f"{tokens['mean']:,.0f} ± {tokens['std']:,.0f}")

    # Input/Output tokens
    input_tokens = agg["tokens_input"]
    output_tokens = agg["tokens_output"]
    if single_run:
        table.add_row("Input Tokens", f"{input_tokens['mean']:,.0f}")
        table.add_row("Output Tokens", f"{output_tokens['mean']:,.0f}")
    else:
        table.add_row("Input Tokens", f"{input_tokens['mean']:,.0f} ± {input_tokens['std']:,.0f}")
        table.add_row("Output Tokens", f"{output_tokens['mean']:,.0f} ± {output_tokens['std']:,.0f}")

    # Tool calls
    calls = agg["tool_calls_total"]
    table.add_row("Tool Calls", _format_stat(calls, single_run))

    # Turns
    turns = agg["turns"]
    table.add_row("Turns", _format_stat(turns, single_run))

    console.print(table)

    # Tool breakdown
    console.print("\n[bold]Tool Usage:[/bold]")
    tools = agg.get("tool_calls", {})

    # Group tools by category
    gabb_tools = sorted([t for t in tools if "gabb" in t.lower()])
    search_tools = sorted([t for t in tools if t in ("Grep", "Glob", "Read")])
    other_tools = sorted([t for t in tools if t not in gabb_tools and t not in search_tools])

    tool_table = Table()
    tool_table.add_column("Tool", style="cyan")
    tool_table.add_column("Count", justify="right")

    for tool in search_tools + gabb_tools + other_tools:
        stats = tools.get(tool, {"mean": 0, "std": 0})
        if stats["mean"] > 0:
            tool_table.add_row(tool, _format_stat(stats, single_run))

    console.print(tool_table)


def _print_single_condition_plain(runs: list[RunMetrics], condition: str) -> None:
    """Print single condition results in plain text."""
    agg = aggregate_runs(runs)
    single_run = len(runs) == 1

    task_id = runs[0].task_id if runs else "Unknown"
    print(f"\n{'=' * 50}")
    title = f"Results: {task_id} ({condition})"
    if not single_run:
        title += f" - {len(runs)} runs"
    print(title)
    print('=' * 50)
    print(f"{'Metric':<20} {'Value':>25}")
    print('-' * 50)

    # Success
    rate = agg["success_rate"]
    if single_run:
        status = "PASS" if rate == 1 else "FAIL"
    else:
        status = f"{rate * 100:.0f}%"
    print(f"{'Success':<20} {status:>25}")

    # Time
    time_stats = agg["wall_time_seconds"]
    print(f"{'Time (s)':<20} {_format_stat(time_stats, single_run):>25}")

    # Tokens
    tokens = agg["tokens_total"]
    if single_run:
        print(f"{'Total Tokens':<20} {tokens['mean']:>25,.0f}")
    else:
        print(f"{'Total Tokens':<20} {tokens['mean']:,.0f} ± {tokens['std']:,.0f}")

    input_tokens = agg["tokens_input"]
    output_tokens = agg["tokens_output"]
    if single_run:
        print(f"{'Input Tokens':<20} {input_tokens['mean']:>25,.0f}")
        print(f"{'Output Tokens':<20} {output_tokens['mean']:>25,.0f}")
    else:
        print(f"{'Input Tokens':<20} {input_tokens['mean']:,.0f} ± {input_tokens['std']:,.0f}")
        print(f"{'Output Tokens':<20} {output_tokens['mean']:,.0f} ± {output_tokens['std']:,.0f}")

    # Tool calls
    calls = agg["tool_calls_total"]
    print(f"{'Tool Calls':<20} {_format_stat(calls, single_run):>25}")

    # Turns
    turns = agg["turns"]
    print(f"{'Turns':<20} {_format_stat(turns, single_run):>25}")

    # Tool breakdown
    print("\nTool Usage:")
    tools = agg.get("tool_calls", {})
    for tool in sorted(tools.keys()):
        stats = tools.get(tool, {"mean": 0, "std": 0})
        if stats["mean"] > 0:
            print(f"  {tool:<35} {_format_stat(stats, single_run):>10}")


def save_results(
    results: dict[str, list[RunMetrics]],
    task_id: str,
    output_dir: Path,
) -> Path:
    """Save results to JSON file.

    Args:
        results: Dict mapping condition name to list of run metrics
        task_id: Task identifier
        output_dir: Directory to save results
    """
    output_dir.mkdir(parents=True, exist_ok=True)
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")

    # Determine run count from any condition
    run_count = max(len(runs) for runs in results.values()) if results else 1
    filepath = output_dir / f"results_{task_id}_n{run_count}_{timestamp}.json"

    data: dict[str, Any] = {
        "task_id": task_id,
        "timestamp": timestamp,
        "run_count": run_count,
        "conditions": {},
    }

    # Build condition data with individual runs and aggregates
    for condition, runs in results.items():
        data["conditions"][condition] = {
            "runs": [r.to_dict() for r in runs],
            "aggregate": aggregate_runs(runs),
        }

    # Add comparison summary if both conditions present
    if "control" in results and "gabb" in results:
        control_agg = aggregate_runs(results["control"])
        gabb_agg = aggregate_runs(results["gabb"])

        c_tokens = control_agg["tokens_total"]["mean"]
        g_tokens = gabb_agg["tokens_total"]["mean"]
        c_time = control_agg["wall_time_seconds"]["mean"]
        g_time = gabb_agg["wall_time_seconds"]["mean"]

        data["summary"] = {
            "token_savings_pct": {
                "mean": round((c_tokens - g_tokens) / max(1, c_tokens) * 100, 1),
            },
            "time_savings_pct": {
                "mean": round((c_time - g_time) / max(0.1, c_time) * 100, 1),
            },
            "control_success_rate": control_agg["success_rate"],
            "gabb_success_rate": gabb_agg["success_rate"],
        }

    with open(filepath, "w") as f:
        json.dump(data, f, indent=2)

    print_msg(f"\nResults saved to {filepath}", "green")
    return filepath


def save_suite_results(
    all_results: list[tuple[list[RunMetrics], list[RunMetrics]]],
    output_dir: Path,
    run_count: int = 1,
) -> Path:
    """Save results from a full suite run.

    Args:
        all_results: List of (control_runs, gabb_runs) tuples, where each is a list of runs
        output_dir: Directory to save results
        run_count: Number of runs per condition
    """
    output_dir.mkdir(parents=True, exist_ok=True)
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    filepath = output_dir / f"suite_results_n{run_count}_{timestamp}.json"

    # Aggregate across all tasks and runs
    all_control_runs: list[RunMetrics] = []
    all_gabb_runs: list[RunMetrics] = []
    for control_runs, gabb_runs in all_results:
        all_control_runs.extend(control_runs)
        all_gabb_runs.extend(gabb_runs)

    control_agg = aggregate_runs(all_control_runs)
    gabb_agg = aggregate_runs(all_gabb_runs)

    # Aggregate tool usage
    control_tools: dict[str, int] = {}
    gabb_tools: dict[str, int] = {}
    for control_runs, gabb_runs in all_results:
        for run in control_runs:
            for tool, count in run.tool_calls.items():
                control_tools[tool] = control_tools.get(tool, 0) + count
        for run in gabb_runs:
            for tool, count in run.tool_calls.items():
                gabb_tools[tool] = gabb_tools.get(tool, 0) + count

    data: dict[str, Any] = {
        "timestamp": timestamp,
        "task_count": len(all_results),
        "run_count": run_count,
        "summary": {
            "control": control_agg,
            "gabb": gabb_agg,
            "token_savings_pct": {
                "mean": round(
                    (control_agg["tokens_total"]["mean"] - gabb_agg["tokens_total"]["mean"])
                    / max(1, control_agg["tokens_total"]["mean"]) * 100,
                    1
                ),
            },
            "time_savings_pct": {
                "mean": round(
                    (control_agg["wall_time_seconds"]["mean"] - gabb_agg["wall_time_seconds"]["mean"])
                    / max(0.1, control_agg["wall_time_seconds"]["mean"]) * 100,
                    1
                ),
            },
            "control_tool_usage": control_tools,
            "gabb_tool_usage": gabb_tools,
        },
        "tasks": [],
    }

    # Add per-task results
    for control_runs, gabb_runs in all_results:
        task_id = control_runs[0].task_id if control_runs else "unknown"
        task_data = {
            "task_id": task_id,
            "control": {
                "runs": [r.to_dict() for r in control_runs],
                "aggregate": aggregate_runs(control_runs),
            },
            "gabb": {
                "runs": [r.to_dict() for r in gabb_runs],
                "aggregate": aggregate_runs(gabb_runs),
            },
        }
        data["tasks"].append(task_data)

    with open(filepath, "w") as f:
        json.dump(data, f, indent=2)

    print_msg(f"\nSuite results saved to {filepath}", "green")
    return filepath


# =============================================================================
# Main
# =============================================================================


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Claude Code Benchmark - Compare performance with/without gabb",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
    # Manual task with workspace
    python run.py --task sklearn-ridge-normalize --workspace /path/to/sklearn

    # SWE-bench task (auto-clones)
    python run.py --swe-bench scikit-learn__scikit-learn-10297

    # Run SWE-bench suite
    python run.py --swe-bench-suite --limit 10 --repo scikit-learn

    # List tasks
    python run.py --list-tasks
    python run.py --list-swe-bench
""",
    )

    # Task selection
    task_group = parser.add_mutually_exclusive_group()
    task_group.add_argument("--task", type=str, help="Manual task ID (requires --workspace)")
    task_group.add_argument("--swe-bench", type=str, help="SWE-bench instance ID")
    task_group.add_argument("--swe-bench-suite", action="store_true", help="Run SWE-bench suite")

    # Workspace
    parser.add_argument("--workspace", type=Path, help="Path to repository (for manual tasks)")
    parser.add_argument("--cache-dir", type=Path, help="Cache directory for cloned repos")

    # Filtering (for suite mode)
    parser.add_argument("--limit", type=int, default=10, help="Max tasks to run in suite mode")
    parser.add_argument("--repo", type=str, help="Filter SWE-bench by repo name")

    # Execution options
    parser.add_argument("--gabb-binary", type=Path, help="Path to gabb binary")
    parser.add_argument(
        "--condition",
        choices=["control", "gabb", "gabb-prompt", "gabb-claudemd", "both", "gabb-all", "all", "full"],
        default="both",
        help="Which condition(s) to run: control, gabb, gabb-prompt (skill via system prompt), "
             "gabb-claudemd (guidance in CLAUDE.md), both=control+gabb, gabb-all=gabb+gabb-prompt, "
             "all=control+gabb+gabb-prompt, full=all 4 conditions",
    )
    parser.add_argument(
        "--runs",
        type=int,
        default=1,
        help="Number of times to run each condition (for statistical significance)",
    )
    parser.add_argument("--no-save", action="store_true", help="Don't save results")
    parser.add_argument("-v", "--verbose", action="store_true", help="Verbose output")

    # Listing
    parser.add_argument("--list-tasks", action="store_true", help="List manual tasks")
    parser.add_argument("--list-swe-bench", action="store_true", help="List SWE-bench tasks")

    args = parser.parse_args()

    if args.verbose:
        logging.basicConfig(level=logging.DEBUG)

    # List manual tasks
    if args.list_tasks:
        tasks = load_manual_tasks()
        if not tasks:
            print_msg("No manual tasks defined.", "yellow")
            return 0
        print_msg("Manual tasks:", "bold")
        for task in tasks:
            print(f"  {task.id}: {task.repo}")
            print(f"    Expected: {task.expected_files}")
        return 0

    # List SWE-bench tasks
    if args.list_swe_bench:
        dataset = load_swe_bench()
        if not dataset:
            return 1

        tasks = list(dataset.iter_tasks(limit=50))
        if args.repo:
            tasks = [t for t in tasks if args.repo.lower() in t.repo.lower()]

        print_msg(f"SWE-bench tasks ({len(tasks)} shown):", "bold")
        for task in tasks[:20]:
            print(f"  {task.instance_id}")
            print(f"    Repo: {task.repo}, Files: {task.gold_files[:2]}")
        if len(tasks) > 20:
            print(f"  ... and {len(tasks) - 20} more")
        return 0

    # Check for gabb binary
    gabb_binary = args.gabb_binary or shutil.which("gabb")
    if args.condition in ("gabb", "gabb-prompt", "gabb-claudemd", "both", "gabb-all", "all", "full") and not gabb_binary:
        print_msg("Warning: gabb binary not found.", "yellow")

    workspace_manager = WorkspaceManager(cache_dir=args.cache_dir)

    # Run SWE-bench suite
    if args.swe_bench_suite:
        dataset = load_swe_bench()
        if not dataset:
            return 1

        tasks = list(dataset.iter_tasks(limit=args.limit))
        if args.repo:
            tasks = [t for t in tasks if args.repo.lower() in t.repo.lower()][:args.limit]

        print_msg(f"Running {len(tasks)} SWE-bench tasks...", "bold")
        if args.runs > 1:
            print_msg(f"Runs per condition: {args.runs}", "dim")

        all_results: list[tuple[list[RunMetrics], list[RunMetrics]]] = []
        for i, swe_task in enumerate(tasks):
            task = swe_bench_task_to_task(swe_task)
            print_msg(f"\n[{i+1}/{len(tasks)}] {task.id}", "bold")

            try:
                workspace = workspace_manager.get_workspace(task.repo, task.base_commit)
                control_runs, gabb_runs = run_comparison(
                    task, workspace, gabb_binary, args.verbose, run_count=args.runs
                )
                all_results.append((control_runs, gabb_runs))
                print_comparison(control_runs, gabb_runs)
            except Exception as e:
                print_msg(f"  Error: {e}", "red")
                continue
            finally:
                # Clean up gabb artifacts between runs
                if 'workspace' in dir():
                    workspace_manager.cleanup_workspace(workspace)

        # Save suite results
        if not args.no_save and all_results:
            save_suite_results(all_results, RESULTS_DIR, run_count=args.runs)

        return 0

    # Run single SWE-bench task
    if args.swe_bench:
        dataset = load_swe_bench()
        if not dataset:
            return 1

        swe_task = dataset.get_task(args.swe_bench)
        if not swe_task:
            print_msg(f"SWE-bench task not found: {args.swe_bench}", "red")
            return 1

        task = swe_bench_task_to_task(swe_task)
        workspace = workspace_manager.get_workspace(task.repo, task.base_commit)
        print_msg(f"Workspace: {workspace}", "dim")

        try:
            if args.condition == "full":
                results = run_full_conditions(
                    task, workspace, gabb_binary, args.verbose, run_count=args.runs
                )
                print_full_conditions(results)
                if not args.no_save:
                    save_results(results, task.id, RESULTS_DIR)
            elif args.condition == "all":
                results = run_all_conditions(
                    task, workspace, gabb_binary, args.verbose, run_count=args.runs
                )
                print_all_conditions(results)
                if not args.no_save:
                    save_results(results, task.id, RESULTS_DIR)
            elif args.condition == "gabb-all":
                results = run_gabb_conditions(
                    task, workspace, gabb_binary, args.verbose, run_count=args.runs
                )
                print_gabb_conditions(results)
                if not args.no_save:
                    save_results(results, task.id, RESULTS_DIR)
            elif args.condition == "both":
                control_runs, gabb_runs = run_comparison(
                    task, workspace, gabb_binary, args.verbose, run_count=args.runs
                )
                print_comparison(control_runs, gabb_runs)
                if not args.no_save:
                    save_results(
                        {"control": control_runs, "gabb": gabb_runs},
                        task.id, RESULTS_DIR
                    )
            else:
                runs = run_multiple(
                    task, workspace, args.condition, args.runs, gabb_binary, args.verbose
                )
                print_single_condition(runs, args.condition)
                if not args.no_save:
                    save_results({args.condition: runs}, task.id, RESULTS_DIR)
        finally:
            workspace_manager.cleanup_workspace(workspace)

        return 0

    # Run manual task
    if args.task:
        if not args.workspace:
            print_msg("Error: --workspace required for manual tasks", "red")
            return 1

        if not args.workspace.exists():
            print_msg(f"Error: Workspace not found: {args.workspace}", "red")
            return 1

        task = get_manual_task(args.task)
        if not task:
            print_msg(f"Task not found: {args.task}", "red")
            available = [t.id for t in load_manual_tasks()]
            if available:
                print_msg(f"Available: {available}", "dim")
            return 1

        print_msg(f"Running: {task.id}", "bold")
        print_msg(f"Workspace: {args.workspace}", "dim")
        if args.runs > 1:
            print_msg(f"Runs: {args.runs}", "dim")

        if args.condition == "full":
            results = run_full_conditions(
                task, args.workspace, gabb_binary, args.verbose, run_count=args.runs
            )
            print_full_conditions(results)
            if not args.no_save:
                save_results(results, task.id, RESULTS_DIR)
        elif args.condition == "all":
            results = run_all_conditions(
                task, args.workspace, gabb_binary, args.verbose, run_count=args.runs
            )
            print_all_conditions(results)
            if not args.no_save:
                save_results(results, task.id, RESULTS_DIR)
        elif args.condition == "gabb-all":
            results = run_gabb_conditions(
                task, args.workspace, gabb_binary, args.verbose, run_count=args.runs
            )
            print_gabb_conditions(results)
            if not args.no_save:
                save_results(results, task.id, RESULTS_DIR)
        elif args.condition == "both":
            control_runs, gabb_runs = run_comparison(
                task, args.workspace, gabb_binary, args.verbose, run_count=args.runs
            )
            print_comparison(control_runs, gabb_runs)
            if not args.no_save:
                save_results(
                    {"control": control_runs, "gabb": gabb_runs},
                    task.id, RESULTS_DIR
                )
        else:
            runs = run_multiple(
                task, args.workspace, args.condition, args.runs, gabb_binary, args.verbose
            )
            print_single_condition(runs, args.condition)
            if not args.no_save:
                save_results({args.condition: runs}, task.id, RESULTS_DIR)

        return 0

    # No task specified
    parser.print_help()
    return 1


if __name__ == "__main__":
    sys.exit(main())
