"""Rule engine for detecting gabb optimization opportunities.

The rule engine provides a pluggable system for detecting patterns in
Claude Code transcripts that could be optimized using gabb tools.

Each rule:
1. Examines tool calls (individual or sequences)
2. Detects patterns that could benefit from gabb
3. Returns Opportunity objects with confidence scores and savings estimates
"""

from __future__ import annotations

from .base import Rule, RuleContext, RuleRegistry
from .grep_to_symbol import GrepToSymbolRule
from .read_to_structure import ReadToStructureRule
from .multi_hop import MultiHopToDefinitionRule
from .find_grep import FindGrepToSymbolsRule

# Register all rules
_registry = RuleRegistry()
_registry.register(GrepToSymbolRule())
_registry.register(ReadToStructureRule())
_registry.register(MultiHopToDefinitionRule())
_registry.register(FindGrepToSymbolsRule())


def get_registry() -> RuleRegistry:
    """Get the global rule registry."""
    return _registry


def detect_opportunities(analysis: "TranscriptAnalysis") -> list["Opportunity"]:
    """Detect all opportunities in a transcript analysis.

    This is the main entry point for opportunity detection.

    Args:
        analysis: The transcript analysis to examine.

    Returns:
        List of detected opportunities, sorted by estimated savings.
    """
    from ..schemas import TranscriptAnalysis

    return _registry.detect_all(analysis)


__all__ = [
    "Rule",
    "RuleContext",
    "RuleRegistry",
    "get_registry",
    "detect_opportunities",
    "GrepToSymbolRule",
    "ReadToStructureRule",
    "MultiHopToDefinitionRule",
    "FindGrepToSymbolsRule",
]
