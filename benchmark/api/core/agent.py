"""Agent implementations for benchmarking."""

from __future__ import annotations

import json
import logging
import re
import time
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from typing import Any

import anthropic

from .env import BenchmarkEnv
from .tools import BaseTool, get_control_tools, get_gabb_tools

logger = logging.getLogger(__name__)


@dataclass
class AgentMetrics:
    """Metrics collected during agent execution."""

    tokens_input: int = 0
    tokens_output: int = 0
    turns: int = 0
    tool_calls: int = 0
    time_seconds: float = 0.0
    final_answer: str | None = None

    # Per-tool metrics
    tool_usage: dict[str, int] = field(default_factory=dict)
    tool_time: dict[str, float] = field(default_factory=dict)

    @property
    def total_tokens(self) -> int:
        return self.tokens_input + self.tokens_output


@dataclass
class AgentConfig:
    """Configuration for an agent."""

    model: str = "claude-sonnet-4-20250514"
    max_turns: int = 20
    max_tokens: int = 4096
    temperature: float = 0.0

    # Early stopping
    stop_on_final_answer: bool = True


RETRIEVAL_SYSTEM_PROMPT = """You are a code navigation assistant. Your task is to identify the file(s) that need to be modified to address a given issue.

You have access to tools that help you explore and search the codebase. Use them efficiently to locate the relevant files.

IMPORTANT RULES:
1. Use search tools strategically - start broad, then narrow down
2. When you find a potentially relevant file, examine its structure and contents
3. Consider the issue description carefully to understand what functionality needs to change
4. Look for the most specific file where the change would be made, not just related files

When you are confident you have found the file(s) that need modification, output your answer in this exact format:
FINAL_ANSWER: <filepath>

For multiple files, list each on a new line:
FINAL_ANSWER: path/to/file1.py
FINAL_ANSWER: path/to/file2.py

Do not include FINAL_ANSWER in your response until you are confident you have found the correct file(s)."""


class BaseAgent(ABC):
    """
    Abstract base class for benchmark agents.

    Manages the Anthropic client conversation loop and tool execution.
    """

    name: str = "base"

    def __init__(
        self,
        env: BenchmarkEnv,
        config: AgentConfig | None = None,
    ):
        """
        Initialize the agent.

        Args:
            env: The benchmark environment for tool execution.
            config: Agent configuration.
        """
        self.env = env
        self.config = config or AgentConfig()
        self._client = anthropic.Anthropic()
        self._tools: list[BaseTool] = []
        self._metrics = AgentMetrics()

    @property
    def metrics(self) -> AgentMetrics:
        """Get the collected metrics."""
        return self._metrics

    @abstractmethod
    def get_tools(self) -> list[BaseTool]:
        """Get the tools available to this agent."""
        pass

    @abstractmethod
    def get_system_prompt(self) -> str:
        """Get the system prompt for this agent."""
        pass

    def _build_tools_schema(self) -> list[dict[str, Any]]:
        """Build the tools schema for the API."""
        return [tool.to_anthropic_tool() for tool in self._tools]

    async def run(self, problem_statement: str) -> AgentMetrics:
        """
        Run the agent on a problem statement.

        Args:
            problem_statement: The issue description to investigate.

        Returns:
            AgentMetrics with performance data and final answer.
        """
        self._tools = self.get_tools()
        self._metrics = AgentMetrics()

        start_time = time.time()
        messages = [{"role": "user", "content": problem_statement}]

        logger.info(f"Starting {self.name} agent run")

        for turn in range(self.config.max_turns):
            self._metrics.turns = turn + 1
            logger.debug(f"Turn {turn + 1}/{self.config.max_turns}")

            # Call the API
            response = await self._call_api(messages)

            # Update token metrics
            self._metrics.tokens_input += response.usage.input_tokens
            self._metrics.tokens_output += response.usage.output_tokens

            # Process response
            assistant_content = []
            tool_use_blocks = []

            for block in response.content:
                if block.type == "text":
                    assistant_content.append({"type": "text", "text": block.text})

                    # Check for final answer
                    final_answer = self._extract_final_answer(block.text)
                    if final_answer:
                        self._metrics.final_answer = final_answer
                        if self.config.stop_on_final_answer:
                            logger.info(f"Final answer found: {final_answer}")
                            self._metrics.time_seconds = time.time() - start_time
                            return self._metrics

                elif block.type == "tool_use":
                    assistant_content.append({
                        "type": "tool_use",
                        "id": block.id,
                        "name": block.name,
                        "input": block.input,
                    })
                    tool_use_blocks.append(block)

            # Add assistant message
            messages.append({"role": "assistant", "content": assistant_content})

            # If no tool calls, we're done
            if not tool_use_blocks:
                logger.info("No more tool calls, agent finished")
                break

            # Execute tools and add results
            tool_results = []
            for tool_block in tool_use_blocks:
                result = await self._execute_tool(tool_block)
                tool_results.append({
                    "type": "tool_result",
                    "tool_use_id": tool_block.id,
                    "content": result,
                })

            messages.append({"role": "user", "content": tool_results})

            # Check for stop reason
            if response.stop_reason == "end_turn":
                break

        self._metrics.time_seconds = time.time() - start_time
        return self._metrics

    async def _call_api(self, messages: list[dict]) -> anthropic.types.Message:
        """Call the Anthropic API."""
        return self._client.messages.create(
            model=self.config.model,
            max_tokens=self.config.max_tokens,
            temperature=self.config.temperature,
            system=self.get_system_prompt(),
            tools=self._build_tools_schema(),
            messages=messages,
        )

    async def _execute_tool(self, tool_block: Any) -> str:
        """Execute a tool and return the result."""
        tool_name = tool_block.name
        tool_input = tool_block.input

        logger.debug(f"Executing tool: {tool_name}")

        # Find the tool
        tool = next((t for t in self._tools if t.name == tool_name), None)
        if not tool:
            return f"Error: Unknown tool '{tool_name}'"

        # Update metrics
        self._metrics.tool_calls += 1
        self._metrics.tool_usage[tool_name] = self._metrics.tool_usage.get(tool_name, 0) + 1

        # Execute
        start_time = time.time()
        try:
            result = await tool.execute(**tool_input)
            elapsed = time.time() - start_time
            self._metrics.tool_time[tool_name] = (
                self._metrics.tool_time.get(tool_name, 0) + elapsed
            )
            return result.to_content()
        except Exception as e:
            logger.error(f"Tool execution error: {e}")
            return f"Error executing {tool_name}: {str(e)}"

    def _extract_final_answer(self, text: str) -> str | None:
        """Extract FINAL_ANSWER from agent output."""
        # Pattern matches "FINAL_ANSWER: path/to/file"
        pattern = r"FINAL_ANSWER:\s*([^\n]+)"
        matches = re.findall(pattern, text)

        if matches:
            # Join multiple answers with newlines
            answers = [m.strip() for m in matches]
            return "\n".join(answers)

        return None


class ControlAgent(BaseAgent):
    """
    Control agent using traditional tools (grep, find, read).

    This serves as the baseline for comparison.
    """

    name = "control"

    def get_tools(self) -> list[BaseTool]:
        """Get grep, find, read, and bash tools."""
        return get_control_tools(self.env)

    def get_system_prompt(self) -> str:
        """Get the system prompt with tool-specific guidance."""
        return RETRIEVAL_SYSTEM_PROMPT + """

AVAILABLE TOOLS:
- grep: Search for patterns in files. Good for finding function/class names.
- find_file: Find files by name pattern. Use to locate files.
- read_file: Read file contents. Use to examine potential matches.
- bash: Run general commands when needed.

STRATEGY:
1. Use grep to search for keywords from the issue description
2. Use find_file if you know part of the filename
3. Use read_file to examine promising files
4. Narrow down to the specific file(s) that need modification"""


class GabbAgent(BaseAgent):
    """
    Agent using gabb semantic indexing tools.

    This is the experimental condition being tested.
    """

    name = "gabb"

    def get_tools(self) -> list[BaseTool]:
        """Get gabb tools plus read_file."""
        return get_gabb_tools(self.env)

    def get_system_prompt(self) -> str:
        """Get the system prompt with gabb-specific guidance."""
        return RETRIEVAL_SYSTEM_PROMPT + """

AVAILABLE TOOLS:
- gabb_symbols: Search for symbols (functions, classes, methods) by name or pattern.
  Much faster and more precise than grep for finding definitions.
- gabb_definition: Jump from a usage to its definition location.
- gabb_structure: Get the structure/outline of a file.
- gabb_usages: Find all places where a symbol is used.
- read_file: Read file contents when you need to see the full code.

STRATEGY:
1. Use gabb_symbols to find relevant functions/classes mentioned in the issue
2. Use gabb_structure to understand file organization
3. Use gabb_definition to trace code flow
4. Use read_file to confirm the file is where the change needs to be made"""


def create_agent(
    agent_type: str,
    env: BenchmarkEnv,
    config: AgentConfig | None = None,
) -> BaseAgent:
    """
    Factory function to create agents.

    Args:
        agent_type: Type of agent ('control' or 'gabb').
        env: Benchmark environment.
        config: Agent configuration.

    Returns:
        The created agent.

    Raises:
        ValueError: If agent_type is unknown.
    """
    agents = {
        "control": ControlAgent,
        "gabb": GabbAgent,
    }

    if agent_type not in agents:
        raise ValueError(f"Unknown agent type: {agent_type}. Choose from: {list(agents.keys())}")

    return agents[agent_type](env, config)
