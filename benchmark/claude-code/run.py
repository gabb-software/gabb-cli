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

    def setup(self) -> None:
        """Set up workspace-local config for Claude Code.

        Uses workspace-local .claude/ directory instead of CLAUDE_CONFIG_DIR
        to preserve authentication credentials stored in system keychain.
        """
        self.temp_dir = Path(tempfile.mkdtemp(prefix="claude_bench_"))
        self.tool_log = self.temp_dir / "tool_calls.jsonl"

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

        # Configure gabb MCP server for gabb condition
        if self.condition == "gabb" and self.gabb_binary:
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

        # Copy SKILL.md for gabb condition
        if self.condition == "gabb":
            skill_src = CONFIGS_DIR / "gabb" / "skills" / "gabb" / "SKILL.md"
            if skill_src.exists():
                skill_dst = self.workspace_claude_dir / "skills" / "gabb"
                skill_dst.mkdir(parents=True, exist_ok=True)
                shutil.copy(skill_src, skill_dst / "SKILL.md")

        # Initialize gabb for gabb condition
        if self.condition == "gabb" and self.gabb_binary:
            self._setup_gabb()

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

        full_prompt = f"""{prompt}

When you find the file(s), output your answer in this format:
FINAL_ANSWER: path/to/file.py

If multiple files, list each on a new line:
FINAL_ANSWER: path/to/file1.py
FINAL_ANSWER: path/to/file2.py"""

        cmd = ["claude", "-p", full_prompt, "--output-format", "json"]

        # Add MCP config for gabb condition
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
        if self.condition == "gabb" and self.gabb_binary:
            subprocess.run(
                [str(self.gabb_binary), "daemon", "stop"],
                cwd=self.workspace,
                capture_output=True,
            )

        if self.temp_dir and self.temp_dir.exists():
            shutil.rmtree(self.temp_dir, ignore_errors=True)

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
) -> RunMetrics:
    """Run a single condition and return metrics."""
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
        return metrics
    finally:
        runner.cleanup()


def run_comparison(
    task: Task,
    workspace: Path,
    gabb_binary: Path | None = None,
    verbose: bool = False,
) -> tuple[RunMetrics, RunMetrics]:
    """Run both conditions on a task."""
    control = run_single_condition(task, workspace, "control", gabb_binary, verbose)
    gabb = run_single_condition(task, workspace, "gabb", gabb_binary, verbose)
    return control, gabb


# =============================================================================
# Output / Reporting
# =============================================================================


def print_comparison(control: RunMetrics, gabb: RunMetrics) -> None:
    """Print comparison of two runs."""
    if HAS_RICH and console:
        _print_comparison_rich(control, gabb)
    else:
        _print_comparison_plain(control, gabb)


def _print_comparison_rich(control: RunMetrics, gabb: RunMetrics) -> None:
    table = Table(title=f"Results: {control.task_id}")
    table.add_column("Metric", style="cyan")
    table.add_column("Control", justify="right")
    table.add_column("Gabb", justify="right")
    table.add_column("Diff", justify="right")

    table.add_row(
        "Success",
        "[green]PASS[/green]" if control.success else "[red]FAIL[/red]",
        "[green]PASS[/green]" if gabb.success else "[red]FAIL[/red]",
        "",
    )

    time_diff = control.wall_time_seconds - gabb.wall_time_seconds
    time_pct = (time_diff / control.wall_time_seconds * 100) if control.wall_time_seconds > 0 else 0
    table.add_row(
        "Time (s)",
        f"{control.wall_time_seconds:.1f}",
        f"{gabb.wall_time_seconds:.1f}",
        f"{time_diff:+.1f} ({time_pct:+.0f}%)",
    )

    control_tokens = control.tokens_input + control.tokens_output
    gabb_tokens = gabb.tokens_input + gabb.tokens_output
    token_diff = control_tokens - gabb_tokens
    token_pct = (token_diff / control_tokens * 100) if control_tokens > 0 else 0
    table.add_row(
        "Total Tokens",
        f"{control_tokens:,}",
        f"{gabb_tokens:,}",
        f"{token_diff:+,} ({token_pct:+.0f}%)",
    )

    control_calls = sum(control.tool_calls.values())
    gabb_calls = sum(gabb.tool_calls.values())
    table.add_row(
        "Tool Calls",
        str(control_calls),
        str(gabb_calls),
        f"{control_calls - gabb_calls:+d}",
    )

    console.print(table)

    # Tool breakdown
    console.print("\n[bold]Tool Usage:[/bold]")
    all_tools = set(control.tool_calls.keys()) | set(gabb.tool_calls.keys())
    gabb_tools = sorted([t for t in all_tools if "gabb" in t.lower()])
    search_tools = sorted([t for t in all_tools if t in ("Grep", "Glob", "Read")])
    other_tools = sorted([t for t in all_tools if t not in gabb_tools and t not in search_tools])

    tool_table = Table()
    tool_table.add_column("Tool", style="cyan")
    tool_table.add_column("Control", justify="right")
    tool_table.add_column("Gabb", justify="right")

    for tool in search_tools + gabb_tools + other_tools:
        c = control.tool_calls.get(tool, 0)
        g = gabb.tool_calls.get(tool, 0)
        if c > 0 or g > 0:
            tool_table.add_row(tool, str(c), str(g))

    console.print(tool_table)


def _print_comparison_plain(control: RunMetrics, gabb: RunMetrics) -> None:
    print(f"\n{'=' * 60}")
    print(f"Results: {control.task_id}")
    print('=' * 60)
    print(f"{'Metric':<20} {'Control':>15} {'Gabb':>15}")
    print('-' * 60)

    c_status = "PASS" if control.success else "FAIL"
    g_status = "PASS" if gabb.success else "FAIL"
    print(f"{'Success':<20} {c_status:>15} {g_status:>15}")
    print(f"{'Time (s)':<20} {control.wall_time_seconds:>15.1f} {gabb.wall_time_seconds:>15.1f}")

    c_tokens = control.tokens_input + control.tokens_output
    g_tokens = gabb.tokens_input + gabb.tokens_output
    print(f"{'Total Tokens':<20} {c_tokens:>15,} {g_tokens:>15,}")

    c_calls = sum(control.tool_calls.values())
    g_calls = sum(gabb.tool_calls.values())
    print(f"{'Tool Calls':<20} {c_calls:>15} {g_calls:>15}")

    print("\nTool Usage:")
    for tool in sorted(set(control.tool_calls.keys()) | set(gabb.tool_calls.keys())):
        c = control.tool_calls.get(tool, 0)
        g = gabb.tool_calls.get(tool, 0)
        if c > 0 or g > 0:
            print(f"  {tool:<30} {c:>10} {g:>10}")


def save_results(results: list[RunMetrics], task_id: str, output_dir: Path) -> Path:
    """Save results to JSON file."""
    output_dir.mkdir(parents=True, exist_ok=True)
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    filepath = output_dir / f"results_{task_id}_{timestamp}.json"

    data = {
        "task_id": task_id,
        "timestamp": timestamp,
        "conditions": {r.condition: r.to_dict() for r in results},
    }

    if len(results) == 2:
        control = next((r for r in results if r.condition == "control"), None)
        gabb = next((r for r in results if r.condition == "gabb"), None)
        if control and gabb:
            c_tokens = control.tokens_input + control.tokens_output
            g_tokens = gabb.tokens_input + gabb.tokens_output
            data["summary"] = {
                "token_savings_pct": round((c_tokens - g_tokens) / max(1, c_tokens) * 100, 1),
                "time_savings_pct": round(
                    (control.wall_time_seconds - gabb.wall_time_seconds)
                    / max(0.1, control.wall_time_seconds) * 100, 1
                ),
                "control_success": control.success,
                "gabb_success": gabb.success,
            }

    with open(filepath, "w") as f:
        json.dump(data, f, indent=2)

    print_msg(f"\nResults saved to {filepath}", "green")
    return filepath


def save_suite_results(all_results: list[tuple[RunMetrics, RunMetrics]], output_dir: Path) -> Path:
    """Save results from a full suite run."""
    output_dir.mkdir(parents=True, exist_ok=True)
    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    filepath = output_dir / f"suite_results_{timestamp}.json"

    # Aggregate metrics
    control_successes = sum(1 for c, g in all_results if c.success)
    gabb_successes = sum(1 for c, g in all_results if g.success)

    total_control_tokens = sum(c.tokens_input + c.tokens_output for c, g in all_results)
    total_gabb_tokens = sum(g.tokens_input + g.tokens_output for c, g in all_results)

    # Aggregate tool usage
    control_tools: dict[str, int] = {}
    gabb_tools: dict[str, int] = {}
    for control, gabb in all_results:
        for tool, count in control.tool_calls.items():
            control_tools[tool] = control_tools.get(tool, 0) + count
        for tool, count in gabb.tool_calls.items():
            gabb_tools[tool] = gabb_tools.get(tool, 0) + count

    data = {
        "timestamp": timestamp,
        "task_count": len(all_results),
        "summary": {
            "control_success_rate": control_successes / len(all_results) if all_results else 0,
            "gabb_success_rate": gabb_successes / len(all_results) if all_results else 0,
            "total_control_tokens": total_control_tokens,
            "total_gabb_tokens": total_gabb_tokens,
            "token_savings_pct": round(
                (total_control_tokens - total_gabb_tokens) / max(1, total_control_tokens) * 100, 1
            ),
            "control_tool_usage": control_tools,
            "gabb_tool_usage": gabb_tools,
        },
        "tasks": [
            {
                "task_id": c.task_id,
                "control": c.to_dict(),
                "gabb": g.to_dict(),
            }
            for c, g in all_results
        ],
    }

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
        choices=["control", "gabb", "both"],
        default="both",
        help="Which condition(s) to run",
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
    if args.condition in ("gabb", "both") and not gabb_binary:
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

        all_results = []
        for i, swe_task in enumerate(tasks):
            task = swe_bench_task_to_task(swe_task)
            print_msg(f"\n[{i+1}/{len(tasks)}] {task.id}", "bold")

            try:
                workspace = workspace_manager.get_workspace(task.repo, task.base_commit)
                control, gabb = run_comparison(task, workspace, gabb_binary, args.verbose)
                all_results.append((control, gabb))
                print_comparison(control, gabb)
            except Exception as e:
                print_msg(f"  Error: {e}", "red")
                continue
            finally:
                # Clean up gabb artifacts between runs
                if 'workspace' in dir():
                    workspace_manager.cleanup_workspace(workspace)

        # Save suite results
        if not args.no_save and all_results:
            save_suite_results(all_results, RESULTS_DIR)

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
            if args.condition == "both":
                control, gabb = run_comparison(task, workspace, gabb_binary, args.verbose)
                print_comparison(control, gabb)
                if not args.no_save:
                    save_results([control, gabb], task.id, RESULTS_DIR)
            else:
                metrics = run_single_condition(task, workspace, args.condition, gabb_binary, args.verbose)
                print_msg(f"\nSuccess: {metrics.success}")
                print_msg(f"Time: {metrics.wall_time_seconds:.1f}s")
                print_msg(f"Tool calls: {metrics.tool_calls}")
                if not args.no_save:
                    save_results([metrics], task.id, RESULTS_DIR)
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

        if args.condition == "both":
            control, gabb = run_comparison(task, args.workspace, gabb_binary, args.verbose)
            print_comparison(control, gabb)
            if not args.no_save:
                save_results([control, gabb], task.id, RESULTS_DIR)
        else:
            metrics = run_single_condition(task, args.workspace, args.condition, gabb_binary, args.verbose)
            print_msg(f"\nSuccess: {metrics.success}")
            if not args.no_save:
                save_results([metrics], task.id, RESULTS_DIR)

        return 0

    # No task specified
    parser.print_help()
    return 1


if __name__ == "__main__":
    sys.exit(main())
