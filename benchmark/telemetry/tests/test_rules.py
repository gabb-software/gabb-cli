"""Tests for opportunity detection rules (Phase 2)."""

from __future__ import annotations

from typing import Optional

import pytest

from gabb_benchmark.schemas import (
    Opportunity,
    OpportunityType,
    BashCommandInfo,
    ToolCall,
    Turn,
    TranscriptAnalysis,
)
from gabb_benchmark.rules import detect_opportunities, get_registry
from gabb_benchmark.rules.base import Rule, RuleContext, RuleRegistry
from gabb_benchmark.rules.grep_to_symbol import GrepToSymbolRule
from gabb_benchmark.rules.read_to_structure import ReadToStructureRule
from gabb_benchmark.rules.multi_hop import MultiHopToDefinitionRule
from gabb_benchmark.rules.find_grep import FindGrepToSymbolsRule


def make_analysis(turns: list[Turn]) -> TranscriptAnalysis:
    """Helper to create a TranscriptAnalysis with given turns."""
    return TranscriptAnalysis(
        session_id="test",
        task_description="Test task",
        turns=turns,
    )


def make_turn(turn_id: int, tool_calls: list[ToolCall]) -> Turn:
    """Helper to create a Turn with given tool calls."""
    return Turn(turn_id=turn_id, tool_calls=tool_calls)


def make_bash_tool_call(
    command: str,
    command_type: str,
    pattern: Optional[str] = None,
    target_path: Optional[str] = None,
    is_recursive: bool = False,
    result_tokens: int = 500,
) -> ToolCall:
    """Helper to create a Bash tool call with parsed info."""
    tc = ToolCall(
        tool_name="Bash",
        tool_input={"command": command},
        tool_use_id="toolu_test",
        result_tokens=result_tokens,
    )
    tc.bash_info = BashCommandInfo(
        raw_command=command,
        command_type=command_type,
        pattern=pattern,
        target_path=target_path,
        is_recursive=is_recursive,
    )
    return tc


def make_read_tool_call(
    file_path: str,
    result_tokens: int = 500,
    offset: Optional[int] = None,
    limit: Optional[int] = None,
) -> ToolCall:
    """Helper to create a Read tool call."""
    tool_input = {"file_path": file_path}
    if offset is not None:
        tool_input["offset"] = offset
    if limit is not None:
        tool_input["limit"] = limit
    return ToolCall(
        tool_name="Read",
        tool_input=tool_input,
        tool_use_id="toolu_test",
        result_tokens=result_tokens,
    )


def make_grep_tool_call(
    pattern: str,
    result_tokens: int = 500,
) -> ToolCall:
    """Helper to create a Grep tool call."""
    return ToolCall(
        tool_name="Grep",
        tool_input={"pattern": pattern},
        tool_use_id="toolu_test",
        result_tokens=result_tokens,
    )


class TestRuleRegistry:
    """Tests for the rule registry."""

    def test_registry_has_rules(self):
        """Test that the registry has registered rules."""
        registry = get_registry()
        rules = registry.get_rules()

        assert len(rules) >= 4  # Our 4 main rules
        rule_names = [r.name for r in rules]
        assert "grep_to_symbol" in rule_names
        assert "read_to_structure" in rule_names
        assert "multi_hop_to_definition" in rule_names
        assert "find_grep_to_symbols" in rule_names

    def test_detect_opportunities_empty_analysis(self):
        """Test detection on empty analysis."""
        analysis = make_analysis([])
        opportunities = detect_opportunities(analysis)

        assert opportunities == []


class TestGrepToSymbolRule:
    """Tests for the GrepToSymbol rule."""

    def test_recursive_grep_for_identifier(self):
        """Test detection of recursive grep for an identifier."""
        tc = make_bash_tool_call(
            command="grep -rn 'handleAuth' src/",
            command_type="grep",
            pattern="handleAuth",
            target_path="src/",
            is_recursive=True,
            result_tokens=1000,
        )
        analysis = make_analysis([make_turn(1, [tc])])

        opportunities = detect_opportunities(analysis)

        assert len(opportunities) >= 1
        opp = opportunities[0]
        assert opp.type == OpportunityType.GREP_TO_USAGES
        assert opp.suggested_tool == "gabb_usages"
        assert opp.estimated_savings > 0
        assert opp.confidence >= 0.7

    def test_non_recursive_grep_for_identifier(self):
        """Test detection of non-recursive grep for an identifier."""
        tc = make_bash_tool_call(
            command="grep 'UserService' file.ts",
            command_type="grep",
            pattern="UserService",
            is_recursive=False,
            result_tokens=500,
        )
        analysis = make_analysis([make_turn(1, [tc])])

        opportunities = detect_opportunities(analysis)

        assert len(opportunities) >= 1
        opp = opportunities[0]
        assert opp.type == OpportunityType.GREP_TO_SYMBOL
        assert opp.suggested_tool == "gabb_symbol"

    def test_grep_non_identifier_no_match(self):
        """Test that grep for non-identifier patterns is not flagged."""
        tc = make_bash_tool_call(
            command="grep 'error:' logs.txt",
            command_type="grep",
            pattern="error:",
            result_tokens=500,
        )
        analysis = make_analysis([make_turn(1, [tc])])

        opportunities = detect_opportunities(analysis)

        # Should not detect opportunity for non-identifier pattern
        grep_opps = [o for o in opportunities if o.type in (
            OpportunityType.GREP_TO_SYMBOL,
            OpportunityType.GREP_TO_USAGES,
            OpportunityType.GREP_TO_SYMBOLS,
        )]
        assert len(grep_opps) == 0

    def test_grep_tool_for_identifier(self):
        """Test detection of Claude's Grep tool for identifier."""
        tc = make_grep_tool_call(pattern="processData", result_tokens=800)
        analysis = make_analysis([make_turn(1, [tc])])

        opportunities = detect_opportunities(analysis)

        assert len(opportunities) >= 1
        opp = opportunities[0]
        assert opp.type == OpportunityType.GREP_TO_USAGES
        assert opp.suggested_tool == "gabb_usages"


class TestReadToStructureRule:
    """Tests for the ReadToStructure rule."""

    def test_full_file_read_large_code_file(self):
        """Test detection of full file read on large code file."""
        tc = make_read_tool_call(
            file_path="src/services/auth.ts",
            result_tokens=1500,
        )
        analysis = make_analysis([make_turn(1, [tc])])

        opportunities = detect_opportunities(analysis)

        assert len(opportunities) >= 1
        opp = opportunities[0]
        assert opp.type == OpportunityType.READ_TO_STRUCTURE
        assert opp.suggested_tool == "gabb_structure"
        assert opp.estimated_savings > 0

    def test_targeted_read_no_match(self):
        """Test that targeted reads with offset/limit are not flagged."""
        tc = make_read_tool_call(
            file_path="src/services/auth.ts",
            result_tokens=500,
            offset=100,
            limit=50,
        )
        analysis = make_analysis([make_turn(1, [tc])])

        opportunities = detect_opportunities(analysis)

        # Should not detect opportunity for targeted read
        read_opps = [o for o in opportunities if o.type == OpportunityType.READ_TO_STRUCTURE]
        assert len(read_opps) == 0

    def test_small_file_read_no_match(self):
        """Test that small file reads are not flagged."""
        tc = make_read_tool_call(
            file_path="src/config.ts",
            result_tokens=200,  # Below threshold
        )
        analysis = make_analysis([make_turn(1, [tc])])

        opportunities = detect_opportunities(analysis)

        read_opps = [o for o in opportunities if o.type == OpportunityType.READ_TO_STRUCTURE]
        assert len(read_opps) == 0

    def test_non_code_file_no_match(self):
        """Test that non-code files are not flagged."""
        tc = make_read_tool_call(
            file_path="README.md",
            result_tokens=1000,
        )
        analysis = make_analysis([make_turn(1, [tc])])

        opportunities = detect_opportunities(analysis)

        read_opps = [o for o in opportunities if o.type == OpportunityType.READ_TO_STRUCTURE]
        assert len(read_opps) == 0


class TestMultiHopRule:
    """Tests for the MultiHopToDefinition rule."""

    def test_grep_then_read_sequence(self):
        """Test detection of grep followed by read."""
        grep_tc = make_bash_tool_call(
            command="grep -rn 'handleAuth' src/",
            command_type="grep",
            pattern="handleAuth",
            is_recursive=True,
            result_tokens=500,
        )
        grep_tc.result_content = "src/auth.ts:42: function handleAuth"

        read_tc = make_read_tool_call(
            file_path="src/auth.ts",
            result_tokens=1000,
        )

        analysis = make_analysis([
            make_turn(1, [grep_tc]),
            make_turn(2, [read_tc]),
        ])

        opportunities = detect_opportunities(analysis)

        multi_hop = [o for o in opportunities if o.type == OpportunityType.MULTI_HOP_TO_DEFINITION]
        assert len(multi_hop) >= 1

    def test_read_without_preceding_grep_no_match(self):
        """Test that standalone reads don't trigger multi-hop detection."""
        read_tc = make_read_tool_call(
            file_path="src/auth.ts",
            result_tokens=1000,
        )

        analysis = make_analysis([make_turn(1, [read_tc])])

        opportunities = detect_opportunities(analysis)

        multi_hop = [o for o in opportunities if o.type == OpportunityType.MULTI_HOP_TO_DEFINITION]
        assert len(multi_hop) == 0


class TestFindGrepRule:
    """Tests for the FindGrepToSymbols rule."""

    def test_find_exec_grep(self):
        """Test detection of find -exec grep pattern."""
        tc = make_bash_tool_call(
            command="find . -name '*.ts' -exec grep -l 'UserService' {} \\;",
            command_type="find",
            pattern="*.ts",
            result_tokens=600,
        )
        analysis = make_analysis([make_turn(1, [tc])])

        opportunities = detect_opportunities(analysis)

        find_grep = [o for o in opportunities if o.type == OpportunityType.FIND_GREP_TO_SYMBOLS]
        assert len(find_grep) >= 1
        assert find_grep[0].suggested_tool == "gabb_symbols"

    def test_glob_then_grep_sequence(self):
        """Test detection of Glob followed by Grep."""
        glob_tc = ToolCall(
            tool_name="Glob",
            tool_input={"pattern": "**/*.ts"},
            tool_use_id="toolu_glob",
            result_tokens=200,
        )

        grep_tc = make_bash_tool_call(
            command="grep 'processData' ",
            command_type="grep",
            pattern="processData",
            result_tokens=500,
        )

        analysis = make_analysis([
            make_turn(1, [glob_tc]),
            make_turn(2, [grep_tc]),
        ])

        opportunities = detect_opportunities(analysis)

        find_grep = [o for o in opportunities if o.type == OpportunityType.FIND_GREP_TO_SYMBOLS]
        assert len(find_grep) >= 1


class TestOpportunityIntegration:
    """Integration tests for opportunity detection."""

    def test_multiple_opportunities_sorted_by_savings(self):
        """Test that opportunities are sorted by estimated savings."""
        # Create multiple tool calls with different savings potential
        tc1 = make_bash_tool_call(
            command="grep -rn 'handleAuth' src/",
            command_type="grep",
            pattern="handleAuth",
            is_recursive=True,
            result_tokens=2000,  # High savings
        )

        tc2 = make_read_tool_call(
            file_path="src/auth.ts",
            result_tokens=500,  # Lower savings
        )

        tc3 = make_bash_tool_call(
            command="grep -rn 'UserService' src/",
            command_type="grep",
            pattern="UserService",
            is_recursive=True,
            result_tokens=1500,  # Medium savings
        )

        analysis = make_analysis([
            make_turn(1, [tc1]),
            make_turn(2, [tc2]),
            make_turn(3, [tc3]),
        ])

        opportunities = detect_opportunities(analysis)

        # Should be sorted by savings (descending)
        if len(opportunities) >= 2:
            for i in range(len(opportunities) - 1):
                assert opportunities[i].estimated_savings >= opportunities[i + 1].estimated_savings

    def test_analysis_opportunities_populated(self):
        """Test that opportunities are properly stored in analysis."""
        tc = make_bash_tool_call(
            command="grep -rn 'handleAuth' src/",
            command_type="grep",
            pattern="handleAuth",
            is_recursive=True,
            result_tokens=1000,
        )
        analysis = make_analysis([make_turn(1, [tc])])
        # Simulate token estimation (normally done by estimate_transcript_tokens)
        analysis.total_input_tokens = 5000
        analysis.total_output_tokens = 2000
        analysis.opportunities = detect_opportunities(analysis)

        data = analysis.to_dict()

        assert "opportunities" in data
        assert data["summary"]["gabb_opportunity_count"] >= 1
        assert data["summary"]["potential_token_savings"] > 0
        assert data["summary"]["savings_percentage"] > 0


class TestRuleContext:
    """Tests for the RuleContext class."""

    def test_get_previous_tool_calls(self):
        """Test getting previous tool calls across turns."""
        tc1 = make_read_tool_call("file1.ts")
        tc2 = make_read_tool_call("file2.ts")
        tc3 = make_read_tool_call("file3.ts")

        analysis = make_analysis([
            make_turn(1, [tc1]),
            make_turn(2, [tc2, tc3]),
        ])

        ctx = RuleContext(analysis=analysis)
        ctx.current_turn_idx = 1
        ctx.current_tool_idx = 1  # tc3

        prev = ctx.get_previous_tool_calls(3)

        # Should get tc2, tc1 (in reverse order)
        assert len(prev) == 2

    def test_current_turn_and_tool(self):
        """Test current_turn and current_tool_call properties."""
        tc = make_read_tool_call("test.ts")
        turn = make_turn(1, [tc])
        analysis = make_analysis([turn])

        ctx = RuleContext(analysis=analysis)
        ctx.current_turn_idx = 0
        ctx.current_tool_idx = 0

        assert ctx.current_turn == turn
        assert ctx.current_tool_call == tc
