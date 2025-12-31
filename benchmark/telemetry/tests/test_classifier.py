"""Tests for tool call classifier."""

import pytest

from gabb_benchmark.classifier import (
    parse_bash_command,
    classify_tool_calls,
    is_identifier_pattern,
)
from gabb_benchmark.parser import parse_transcript


class TestBashCommandParser:
    """Tests for parse_bash_command."""

    def test_parse_grep_simple(self):
        """Test parsing a simple grep command."""
        info = parse_bash_command("grep 'pattern' file.txt")

        assert info.command_type == "grep"
        assert info.pattern == "pattern"
        assert info.target_path == "file.txt"
        assert not info.is_recursive

    def test_parse_grep_recursive(self):
        """Test parsing a recursive grep command."""
        info = parse_bash_command("grep -rn 'handleAuth' src/")

        assert info.command_type == "grep"
        assert info.pattern == "handleAuth"
        assert info.target_path == "src/"
        assert info.is_recursive
        assert "-rn" in info.flags

    def test_parse_grep_with_e_flag(self):
        """Test parsing grep with -e flag."""
        info = parse_bash_command("grep -e 'pattern1' -e 'pattern2' file.txt")

        assert info.command_type == "grep"
        # Should capture first pattern
        assert info.pattern == "pattern1"

    def test_parse_find(self):
        """Test parsing a find command."""
        info = parse_bash_command("find . -name '*.ts' -type f")

        assert info.command_type == "find"
        assert info.pattern == "*.ts"
        assert info.target_path == "."
        assert info.is_recursive

    def test_parse_cat(self):
        """Test parsing a cat command."""
        info = parse_bash_command("cat src/auth.ts")

        assert info.command_type == "cat"
        assert info.target_path == "src/auth.ts"

    def test_parse_head(self):
        """Test parsing a head command."""
        info = parse_bash_command("head -n 50 src/auth.ts")

        assert info.command_type == "head"
        assert info.target_path == "src/auth.ts"
        assert "-n" in info.flags

    def test_parse_git_status(self):
        """Test parsing a git command."""
        info = parse_bash_command("git status")

        assert info.command_type == "git-status"

    def test_parse_git_log(self):
        """Test parsing git log with args."""
        info = parse_bash_command("git log --oneline -10")

        assert info.command_type == "git-log"

    def test_parse_ripgrep(self):
        """Test parsing rg (ripgrep) as grep."""
        info = parse_bash_command("rg 'pattern' src/")

        assert info.command_type == "grep"
        assert info.pattern == "pattern"

    def test_parse_empty_command(self):
        """Test parsing empty command."""
        info = parse_bash_command("")

        assert info.command_type == "empty"

    def test_parse_npm_install(self):
        """Test parsing npm command."""
        info = parse_bash_command("npm install express")

        assert info.command_type == "npm"

    def test_parse_cargo_build(self):
        """Test parsing cargo command."""
        info = parse_bash_command("cargo build --release")

        assert info.command_type == "cargo"


class TestIdentifierPattern:
    """Tests for is_identifier_pattern."""

    def test_pascal_case(self):
        """Test PascalCase identifiers."""
        assert is_identifier_pattern("UserService")
        assert is_identifier_pattern("HandleAuth")

    def test_camel_case(self):
        """Test camelCase identifiers."""
        assert is_identifier_pattern("handleAuth")
        assert is_identifier_pattern("getUserById")

    def test_snake_case(self):
        """Test snake_case identifiers."""
        assert is_identifier_pattern("handle_auth")
        assert is_identifier_pattern("get_user_by_id")

    def test_screaming_snake(self):
        """Test SCREAMING_SNAKE_CASE identifiers."""
        assert is_identifier_pattern("MAX_RETRIES")
        assert is_identifier_pattern("API_KEY")

    def test_not_identifier(self):
        """Test patterns that are not identifiers."""
        assert not is_identifier_pattern("this is a phrase")
        assert not is_identifier_pattern("123")
        assert not is_identifier_pattern("")
        assert not is_identifier_pattern("a")  # Too short


class TestClassifyToolCalls:
    """Tests for classify_tool_calls."""

    def test_classify_bash_grep(self):
        """Test classifying a Bash grep command."""
        data = {
            "messages": [
                {"role": "user", "content": "Find it"},
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "toolu_1",
                            "name": "Bash",
                            "input": {"command": "grep -r 'pattern' src/"},
                        }
                    ],
                },
            ]
        }
        analysis = parse_transcript(data)
        classify_tool_calls(analysis)

        tc = analysis.turns[0].tool_calls[0]
        assert tc.bash_info is not None
        assert tc.bash_info.command_type == "grep"
        assert tc.bash_info.pattern == "pattern"
        assert tc.bash_info.is_recursive

    def test_classify_non_bash_tool(self):
        """Test that non-Bash tools don't get bash_info."""
        data = {
            "messages": [
                {"role": "user", "content": "Read it"},
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "toolu_1",
                            "name": "Read",
                            "input": {"file_path": "test.txt"},
                        }
                    ],
                },
            ]
        }
        analysis = parse_transcript(data)
        classify_tool_calls(analysis)

        tc = analysis.turns[0].tool_calls[0]
        assert tc.bash_info is None
