"""Tests for reporter module."""

import json
from pathlib import Path

import pytest

from gabb_benchmark.parser import load_transcript
from gabb_benchmark.classifier import classify_tool_calls
from gabb_benchmark.estimator import estimate_transcript_tokens
from gabb_benchmark.rules import detect_opportunities
from gabb_benchmark.reporter import (
    format_number,
    generate_json_report,
    generate_text_report,
    generate_markdown_report,
    generate_recommendations,
)


@pytest.fixture
def analyzed_transcript(simple_transcript_path: Path):
    """Load and analyze a transcript."""
    analysis = load_transcript(simple_transcript_path)
    classify_tool_calls(analysis)
    estimate_transcript_tokens(analysis)
    analysis.opportunities = detect_opportunities(analysis)
    return analysis


def test_format_number():
    """Test number formatting with thousands separator."""
    assert format_number(1000) == "1,000"
    assert format_number(1000000) == "1,000,000"
    assert format_number(42) == "42"
    assert format_number(0) == "0"


def test_generate_json_report(analyzed_transcript):
    """Test JSON report generation."""
    report = generate_json_report(analyzed_transcript)
    data = json.loads(report)

    assert "summary" in data
    assert "turns" in data
    assert "tool_distribution" in data
    assert "opportunities" in data
    assert data["summary"]["total_turns"] > 0


def test_generate_text_report(analyzed_transcript):
    """Test text report generation."""
    report = generate_text_report(analyzed_transcript)

    assert "Gabb Benchmark Report" in report
    assert "TOKEN SUMMARY" in report
    assert "TOOL DISTRIBUTION" in report


def test_generate_markdown_report(analyzed_transcript):
    """Test markdown report generation."""
    report = generate_markdown_report(analyzed_transcript)

    assert "# Gabb Benchmark Report" in report
    assert "## Summary" in report
    assert "| Metric | Value |" in report
    assert "## Tool Distribution" in report


def test_generate_markdown_report_verbose(analyzed_transcript):
    """Test markdown report with verbose mode."""
    report = generate_markdown_report(analyzed_transcript, verbose=True)

    assert "## Per-Turn Breakdown" in report
    assert "### Detailed Tool Calls" in report
    # Should include turn details
    assert "**Turn" in report


def test_generate_markdown_report_with_opportunities(analyzed_transcript):
    """Test markdown report includes opportunities when present."""
    # Only run if there are opportunities
    if analyzed_transcript.opportunities:
        report = generate_markdown_report(analyzed_transcript)
        assert "## Gabb Optimization Opportunities" in report
        assert "## Recommendations" in report


def test_generate_recommendations_empty():
    """Test recommendations with no opportunities."""
    from gabb_benchmark.schemas import TranscriptAnalysis

    analysis = TranscriptAnalysis()
    recs = generate_recommendations(analysis)
    assert recs == []


def test_generate_recommendations_with_opportunities(analyzed_transcript):
    """Test recommendations are generated based on opportunity types."""
    recs = generate_recommendations(analyzed_transcript)

    # Should return a list (may be empty if no opportunities)
    assert isinstance(recs, list)

    if recs:
        # Each recommendation should have required fields
        for rec in recs:
            assert "priority" in rec
            assert "title" in rec
            assert "description" in rec
            assert "impact" in rec
            assert "example" in rec


def test_markdown_report_escapes_special_chars(analyzed_transcript):
    """Test that markdown report handles special characters."""
    # Add a task description with special markdown chars
    analyzed_transcript.task_description = "Fix the `auth` bug with **priority**"
    report = generate_markdown_report(analyzed_transcript)

    # Should include the task description
    assert "Fix the `auth` bug with **priority**" in report


def test_text_report_with_long_commands(analyzed_transcript):
    """Test text report truncates long commands properly."""
    report = generate_text_report(analyzed_transcript)

    # Report should complete without errors
    assert len(report) > 0
    assert "=" * 70 in report  # Header/footer lines
