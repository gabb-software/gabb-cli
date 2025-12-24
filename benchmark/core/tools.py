"""Tool definitions for benchmark agents."""

from __future__ import annotations

import json
from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Any, TYPE_CHECKING

if TYPE_CHECKING:
    from .env import BenchmarkEnv


@dataclass
class ToolResult:
    """Result from a tool execution."""

    success: bool
    output: str
    error: str | None = None
    truncated: bool = False

    def to_content(self) -> str:
        """Convert to content string for the agent."""
        if not self.success:
            return f"Error: {self.error or self.output}"
        return self.output


class BaseTool(ABC):
    """Base class for all tools."""

    name: str
    description: str

    def __init__(self, env: "BenchmarkEnv"):
        """
        Initialize the tool.

        Args:
            env: The benchmark environment to execute in.
        """
        self.env = env

    @abstractmethod
    def get_schema(self) -> dict[str, Any]:
        """Get the JSON schema for the tool parameters."""
        pass

    @abstractmethod
    async def execute(self, **kwargs) -> ToolResult:
        """Execute the tool with the given parameters."""
        pass

    def to_anthropic_tool(self) -> dict[str, Any]:
        """Convert to Anthropic tool format."""
        return {
            "name": self.name,
            "description": self.description,
            "input_schema": self.get_schema(),
        }


# ============================================================================
# Control Tools (grep, find, read)
# ============================================================================


class GrepTool(BaseTool):
    """Search for patterns in files using grep."""

    name = "grep"
    description = """Search for a pattern in files using grep.
Use this to find occurrences of text patterns, function names, class names, etc.
Returns matching lines with file paths and line numbers."""

    def get_schema(self) -> dict[str, Any]:
        return {
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The pattern to search for (supports regex)",
                },
                "path": {
                    "type": "string",
                    "description": "Directory or file path to search in (default: current directory)",
                    "default": ".",
                },
                "include": {
                    "type": "string",
                    "description": "File pattern to include (e.g., '*.py')",
                },
                "context": {
                    "type": "integer",
                    "description": "Number of context lines before and after matches",
                    "default": 0,
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return",
                    "default": 50,
                },
            },
            "required": ["pattern"],
        }

    async def execute(
        self,
        pattern: str,
        path: str = ".",
        include: str | None = None,
        context: int = 0,
        max_results: int = 50,
    ) -> ToolResult:
        """Execute grep search."""
        cmd_parts = ["grep", "-rn"]

        if context > 0:
            cmd_parts.append(f"-C{context}")

        if include:
            cmd_parts.extend(["--include", include])

        cmd_parts.extend(["-e", f"'{pattern}'", path])
        cmd_parts.append(f"| head -n {max_results}")

        cmd = " ".join(cmd_parts)
        result = await self.env.exec(cmd)

        if result.exit_code == 1 and not result.stdout:
            # grep returns 1 when no matches found
            return ToolResult(success=True, output="No matches found.")

        if result.exit_code not in (0, 1):
            return ToolResult(success=False, output="", error=result.stderr)

        output = result.stdout.strip()
        truncated = len(output.split("\n")) >= max_results

        return ToolResult(success=True, output=output, truncated=truncated)


class FindFileTool(BaseTool):
    """Find files by name pattern."""

    name = "find_file"
    description = """Find files by name pattern.
Use this to locate files when you know part of the filename.
Returns a list of matching file paths."""

    def get_schema(self) -> dict[str, Any]:
        return {
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "File name pattern to search for (e.g., '*.py', 'test_*.py', '*handler*')",
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: current directory)",
                    "default": ".",
                },
                "type": {
                    "type": "string",
                    "enum": ["f", "d", "any"],
                    "description": "Type of item to find: 'f' for files, 'd' for directories, 'any' for both",
                    "default": "f",
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return",
                    "default": 50,
                },
            },
            "required": ["pattern"],
        }

    async def execute(
        self,
        pattern: str,
        path: str = ".",
        type: str = "f",
        max_results: int = 50,
    ) -> ToolResult:
        """Execute find search."""
        cmd_parts = ["find", path]

        if type in ("f", "d"):
            cmd_parts.extend(["-type", type])

        cmd_parts.extend(["-name", f"'{pattern}'"])
        cmd_parts.append(f"| head -n {max_results}")

        cmd = " ".join(cmd_parts)
        result = await self.env.exec(cmd)

        if not result.success:
            return ToolResult(success=False, output="", error=result.stderr)

        output = result.stdout.strip()
        if not output:
            return ToolResult(success=True, output="No files found matching pattern.")

        truncated = len(output.split("\n")) >= max_results
        return ToolResult(success=True, output=output, truncated=truncated)


class ReadFileTool(BaseTool):
    """Read contents of a file."""

    name = "read_file"
    description = """Read the contents of a file.
Use this to examine the source code of a specific file.
Supports reading specific line ranges to avoid overwhelming output."""

    def get_schema(self) -> dict[str, Any]:
        return {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read",
                },
                "start_line": {
                    "type": "integer",
                    "description": "Starting line number (1-indexed)",
                    "default": 1,
                },
                "end_line": {
                    "type": "integer",
                    "description": "Ending line number (inclusive). Use -1 for end of file.",
                    "default": -1,
                },
                "max_lines": {
                    "type": "integer",
                    "description": "Maximum number of lines to return",
                    "default": 200,
                },
            },
            "required": ["path"],
        }

    async def execute(
        self,
        path: str,
        start_line: int = 1,
        end_line: int = -1,
        max_lines: int = 200,
    ) -> ToolResult:
        """Read file contents."""
        # First check if file exists
        check_result = await self.env.exec(f"test -f '{path}' && echo 'exists'")
        if "exists" not in check_result.stdout:
            return ToolResult(
                success=False,
                output="",
                error=f"File not found: {path}",
            )

        # Build the read command
        if start_line == 1 and end_line == -1:
            cmd = f"head -n {max_lines} '{path}'"
        elif end_line == -1:
            cmd = f"tail -n +{start_line} '{path}' | head -n {max_lines}"
        else:
            lines_to_read = min(end_line - start_line + 1, max_lines)
            cmd = f"sed -n '{start_line},{start_line + lines_to_read - 1}p' '{path}'"

        result = await self.env.exec(cmd)

        if not result.success:
            return ToolResult(success=False, output="", error=result.stderr)

        # Add line numbers
        lines = result.stdout.split("\n")
        numbered_lines = []
        for i, line in enumerate(lines, start=start_line):
            numbered_lines.append(f"{i:5d} | {line}")

        output = "\n".join(numbered_lines)
        truncated = len(lines) >= max_lines

        return ToolResult(success=True, output=output, truncated=truncated)


class BashTool(BaseTool):
    """Execute arbitrary bash commands."""

    name = "bash"
    description = """Execute a bash command.
Use this for general-purpose commands when other tools don't fit.
Be careful with destructive commands."""

    def get_schema(self) -> dict[str, Any]:
        return {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute",
                },
                "timeout": {
                    "type": "integer",
                    "description": "Command timeout in seconds",
                    "default": 30,
                },
            },
            "required": ["command"],
        }

    async def execute(self, command: str, timeout: int = 30) -> ToolResult:
        """Execute bash command."""
        result = await self.env.exec(command, timeout=timeout)

        if not result.success:
            return ToolResult(
                success=False,
                output=result.stdout,
                error=result.stderr,
            )

        return ToolResult(success=True, output=result.output)


# ============================================================================
# Gabb Tools
# ============================================================================


class GabbSymbolsTool(BaseTool):
    """Search for symbols using gabb."""

    name = "gabb_symbols"
    description = """Search for code symbols (functions, classes, methods, etc.) using gabb semantic index.
This is much faster and more precise than grep for finding symbol definitions.
Supports filtering by name pattern, symbol kind, and file path."""

    def get_schema(self) -> dict[str, Any]:
        return {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Exact symbol name to search for",
                },
                "name_pattern": {
                    "type": "string",
                    "description": "Glob pattern for symbol name (e.g., 'get*', '*Handler')",
                },
                "name_contains": {
                    "type": "string",
                    "description": "Substring to search for in symbol names",
                },
                "kind": {
                    "type": "string",
                    "description": "Symbol kind: function, class, method, interface, type, struct, enum, trait",
                },
                "file": {
                    "type": "string",
                    "description": "Filter by file path pattern (e.g., 'src/**/*.py')",
                },
                "include_source": {
                    "type": "boolean",
                    "description": "Include source code snippet in output",
                    "default": False,
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results",
                    "default": 50,
                },
            },
        }

    async def execute(
        self,
        name: str | None = None,
        name_pattern: str | None = None,
        name_contains: str | None = None,
        kind: str | None = None,
        file: str | None = None,
        include_source: bool = False,
        limit: int = 50,
    ) -> ToolResult:
        """Execute gabb symbols search."""
        cmd_parts = ["gabb", "symbols", "--db", "/workspace/.gabb/index.db", "--json"]

        if name:
            cmd_parts.extend(["--name", name])
        if name_pattern:
            cmd_parts.extend(["--name-pattern", name_pattern])
        if name_contains:
            cmd_parts.extend(["--name-contains", name_contains])
        if kind:
            cmd_parts.extend(["--kind", kind])
        if file:
            cmd_parts.extend(["--file", file])
        if include_source:
            cmd_parts.append("--include-source")

        cmd_parts.extend(["--limit", str(limit)])

        cmd = " ".join(cmd_parts)
        result = await self.env.exec(cmd)

        if not result.success:
            return ToolResult(success=False, output="", error=result.stderr)

        # Parse and format JSON output
        try:
            symbols = json.loads(result.stdout)
            if not symbols:
                return ToolResult(success=True, output="No symbols found.")

            # Format output
            lines = []
            for sym in symbols:
                loc = f"{sym['file']}:{sym['line']}:{sym['character']}"
                lines.append(f"{sym['kind']:12} {sym['name']:40} {loc}")
                if include_source and "source" in sym:
                    lines.append(f"    {sym['source'][:100]}")

            return ToolResult(success=True, output="\n".join(lines))
        except json.JSONDecodeError:
            return ToolResult(success=True, output=result.stdout)


class GabbDefinitionTool(BaseTool):
    """Go to definition using gabb."""

    name = "gabb_definition"
    description = """Jump to the definition of a symbol at a specific location.
Use this when you see a function call, type reference, or variable and want to find where it's defined.
Point to the symbol usage location, and this returns its definition."""

    def get_schema(self) -> dict[str, Any]:
        return {
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "Path to the file containing the symbol usage",
                },
                "line": {
                    "type": "integer",
                    "description": "Line number (1-indexed)",
                },
                "character": {
                    "type": "integer",
                    "description": "Character/column position (1-indexed)",
                },
                "include_source": {
                    "type": "boolean",
                    "description": "Include source code at definition",
                    "default": True,
                },
            },
            "required": ["file", "line", "character"],
        }

    async def execute(
        self,
        file: str,
        line: int,
        character: int,
        include_source: bool = True,
    ) -> ToolResult:
        """Execute gabb definition lookup."""
        cmd_parts = [
            "gabb", "definition",
            "--db", "/workspace/.gabb/index.db",
            "--file", f"{file}:{line}:{character}",
            "--json",
        ]

        if include_source:
            cmd_parts.append("--include-source")

        cmd = " ".join(cmd_parts)
        result = await self.env.exec(cmd)

        if not result.success:
            return ToolResult(success=False, output="", error=result.stderr)

        try:
            data = json.loads(result.stdout)
            if not data:
                return ToolResult(success=True, output="Definition not found.")

            loc = f"{data['file']}:{data['line']}:{data['character']}"
            output = f"Definition: {data['name']} ({data['kind']}) at {loc}"
            if include_source and "source" in data:
                output += f"\n\n{data['source']}"

            return ToolResult(success=True, output=output)
        except json.JSONDecodeError:
            return ToolResult(success=True, output=result.stdout)


class GabbStructureTool(BaseTool):
    """Get file structure using gabb."""

    name = "gabb_structure"
    description = """Get the structure of a file showing all symbols.
Use this to understand a file's organization before reading it in full.
Returns symbols grouped hierarchically with start/end positions."""

    def get_schema(self) -> dict[str, Any]:
        return {
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "Path to the file to analyze",
                },
                "include_source": {
                    "type": "boolean",
                    "description": "Include source code snippets",
                    "default": False,
                },
            },
            "required": ["file"],
        }

    async def execute(
        self,
        file: str,
        include_source: bool = False,
    ) -> ToolResult:
        """Execute gabb structure analysis."""
        cmd_parts = [
            "gabb", "structure",
            "--db", "/workspace/.gabb/index.db",
            "--file", file,
            "--json",
        ]

        if include_source:
            cmd_parts.append("--include-source")

        cmd = " ".join(cmd_parts)
        result = await self.env.exec(cmd)

        if not result.success:
            return ToolResult(success=False, output="", error=result.stderr)

        try:
            data = json.loads(result.stdout)
            if not data:
                return ToolResult(success=True, output="No symbols found in file.")

            # Format hierarchically
            lines = [f"Structure of {file}:", ""]
            for sym in data:
                indent = "  " * sym.get("depth", 0)
                lines.append(f"{indent}{sym['kind']:12} {sym['name']} (L{sym['line']})")

            return ToolResult(success=True, output="\n".join(lines))
        except json.JSONDecodeError:
            return ToolResult(success=True, output=result.stdout)


class GabbUsagesTool(BaseTool):
    """Find usages of a symbol using gabb."""

    name = "gabb_usages"
    description = """Find all places where a symbol is used/referenced.
Use this to understand how a function is called or where a class is instantiated.
More accurate than grep - understands code structure."""

    def get_schema(self) -> dict[str, Any]:
        return {
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "Path to the file containing the symbol definition",
                },
                "line": {
                    "type": "integer",
                    "description": "Line number of the symbol (1-indexed)",
                },
                "character": {
                    "type": "integer",
                    "description": "Character/column position (1-indexed)",
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of usages to return",
                    "default": 50,
                },
            },
            "required": ["file", "line", "character"],
        }

    async def execute(
        self,
        file: str,
        line: int,
        character: int,
        limit: int = 50,
    ) -> ToolResult:
        """Execute gabb usages search."""
        cmd_parts = [
            "gabb", "usages",
            "--db", "/workspace/.gabb/index.db",
            "--file", f"{file}:{line}:{character}",
            "--limit", str(limit),
            "--json",
        ]

        cmd = " ".join(cmd_parts)
        result = await self.env.exec(cmd)

        if not result.success:
            return ToolResult(success=False, output="", error=result.stderr)

        try:
            usages = json.loads(result.stdout)
            if not usages:
                return ToolResult(success=True, output="No usages found.")

            lines = [f"Found {len(usages)} usages:", ""]
            for usage in usages:
                loc = f"{usage['file']}:{usage['line']}:{usage['character']}"
                lines.append(f"  {loc}")

            return ToolResult(success=True, output="\n".join(lines))
        except json.JSONDecodeError:
            return ToolResult(success=True, output=result.stdout)


# ============================================================================
# Tool Sets
# ============================================================================


def get_control_tools(env: "BenchmarkEnv") -> list[BaseTool]:
    """Get the control agent tool set (grep, find, read)."""
    return [
        GrepTool(env),
        FindFileTool(env),
        ReadFileTool(env),
        BashTool(env),
    ]


def get_gabb_tools(env: "BenchmarkEnv") -> list[BaseTool]:
    """Get the gabb agent tool set."""
    return [
        GabbSymbolsTool(env),
        GabbDefinitionTool(env),
        GabbStructureTool(env),
        GabbUsagesTool(env),
        ReadFileTool(env),  # Still need read for examining specific code
    ]
