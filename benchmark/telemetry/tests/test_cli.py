"""Tests for CLI."""

import json
from pathlib import Path

import pytest

from gabb_benchmark.cli import main


def test_cli_help():
    """Test --help flag."""
    with pytest.raises(SystemExit) as exc_info:
        main(["--help"])
    assert exc_info.value.code == 0


def test_cli_version():
    """Test --version flag."""
    with pytest.raises(SystemExit) as exc_info:
        main(["--version"])
    assert exc_info.value.code == 0


def test_cli_no_command():
    """Test running with no command shows help."""
    result = main([])
    assert result == 1


def test_cli_analyze_missing_file():
    """Test analyze with missing file."""
    result = main(["analyze", "nonexistent.json"])
    assert result == 1


def test_cli_analyze_simple(simple_transcript_path: Path):
    """Test analyzing a simple transcript."""
    result = main(["analyze", str(simple_transcript_path), "--format", "text"])
    assert result == 0


def test_cli_analyze_json_output(simple_transcript_path: Path, capsys):
    """Test JSON output format."""
    result = main(["analyze", str(simple_transcript_path), "--format", "json"])
    assert result == 0

    captured = capsys.readouterr()
    output = json.loads(captured.out)

    assert "summary" in output
    assert "turns" in output
    assert "tool_distribution" in output


def test_cli_analyze_multiple_files(simple_transcript_path: Path, multi_tool_transcript_path: Path):
    """Test analyzing multiple files."""
    result = main([
        "analyze",
        str(simple_transcript_path),
        str(multi_tool_transcript_path),
        "--format", "text",
    ])
    assert result == 0


def test_cli_analyze_summary(simple_transcript_path: Path, multi_tool_transcript_path: Path, capsys):
    """Test --summary flag aggregates results."""
    result = main([
        "analyze",
        str(simple_transcript_path),
        str(multi_tool_transcript_path),
        "--summary",
        "--format", "text",
    ])
    assert result == 0

    captured = capsys.readouterr()
    assert "AGGREGATE SUMMARY" in captured.out
    assert "Transcripts analyzed:" in captured.out


def test_cli_analyze_markdown_output(simple_transcript_path: Path, capsys):
    """Test markdown output format."""
    result = main(["analyze", str(simple_transcript_path), "--format", "markdown"])
    assert result == 0

    captured = capsys.readouterr()
    # Check for markdown headings and tables
    assert "# Gabb Benchmark Report" in captured.out
    assert "## Summary" in captured.out
    assert "| Metric | Value |" in captured.out
    assert "## Tool Distribution" in captured.out


def test_cli_analyze_markdown_verbose(simple_transcript_path: Path, capsys):
    """Test markdown output with verbose mode."""
    result = main([
        "analyze",
        str(simple_transcript_path),
        "--format", "markdown",
        "--verbose",
    ])
    assert result == 0

    captured = capsys.readouterr()
    assert "## Per-Turn Breakdown" in captured.out
    assert "### Detailed Tool Calls" in captured.out


def test_cli_analyze_summary_markdown(simple_transcript_path: Path, multi_tool_transcript_path: Path, capsys):
    """Test --summary with markdown format."""
    result = main([
        "analyze",
        str(simple_transcript_path),
        str(multi_tool_transcript_path),
        "--summary",
        "--format", "markdown",
    ])
    assert result == 0

    captured = capsys.readouterr()
    assert "# Aggregate Summary" in captured.out
    assert "## Overview" in captured.out
    assert "| Metric | Value |" in captured.out


def test_cli_analyze_verbose_rich(simple_transcript_path: Path, capsys):
    """Test verbose mode with rich output."""
    result = main([
        "analyze",
        str(simple_transcript_path),
        "--format", "rich",
        "--verbose",
    ])
    assert result == 0
    # Rich output is printed to console, so just check it doesn't error
