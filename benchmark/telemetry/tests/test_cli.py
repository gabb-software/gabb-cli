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
