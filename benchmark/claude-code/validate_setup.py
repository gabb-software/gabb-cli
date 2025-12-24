#!/usr/bin/env python3
"""
Validate that the benchmark setup correctly configures gabb MCP server.

This script:
1. Sets up a test workspace like the benchmark does
2. Verifies the settings.local.json is correct
3. Verifies the gabb daemon starts and indexes files
4. Runs a simple Claude prompt to check if MCP tools are available
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

BENCHMARK_DIR = Path(__file__).parent
CONFIGS_DIR = BENCHMARK_DIR / "configs"
HOOKS_DIR = BENCHMARK_DIR / "hooks"
API_ENV_FILE = BENCHMARK_DIR.parent / "api" / ".env"


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


def main():
    print("=" * 60)
    print("Benchmark Setup Validation")
    print("=" * 60)

    # Step 1: Check gabb binary
    print("\n[1] Checking gabb binary...")
    gabb_binary = shutil.which("gabb")
    if not gabb_binary:
        print("  ❌ gabb binary not found in PATH")
        return 1
    print(f"  ✓ Found: {gabb_binary}")

    result = subprocess.run([gabb_binary, "--version"], capture_output=True, text=True)
    print(f"  ✓ Version: {result.stdout.strip()}")

    # Step 2: Create a test workspace
    print("\n[2] Creating test workspace...")
    test_dir = Path(tempfile.mkdtemp(prefix="gabb_validate_"))
    print(f"  ✓ Created: {test_dir}")

    # Create a simple Python file to index
    # Create a .git marker so gabb recognizes this as a project
    (test_dir / ".git").mkdir()

    (test_dir / "example.py").write_text('''
def hello_world():
    """A simple greeting function."""
    return "Hello, World!"

class UserService:
    """Service for user operations."""

    def get_user(self, user_id: int):
        """Get a user by ID."""
        return {"id": user_id, "name": "Test User"}

    def create_user(self, name: str):
        """Create a new user."""
        return {"id": 1, "name": name}
''')
    print("  ✓ Created example.py with test code")

    # Step 3: Initialize gabb
    print("\n[3] Initializing gabb...")
    result = subprocess.run(
        [gabb_binary, "init"],
        cwd=test_dir,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print(f"  ❌ gabb init failed: {result.stderr}")
        return 1
    print("  ✓ gabb init succeeded")

    # Step 4: Start daemon and wait for indexing
    print("\n[4] Starting gabb daemon...")
    result = subprocess.run(
        [gabb_binary, "daemon", "start", "-b"],
        cwd=test_dir,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print(f"  ⚠ daemon start returned: {result.returncode}")
        print(f"    stdout: {result.stdout[:200] if result.stdout else '(empty)'}")
        print(f"    stderr: {result.stderr[:200] if result.stderr else '(empty)'}")
    else:
        print("  ✓ Daemon started in background")

    # Wait for indexing to complete
    print("  Waiting for indexing...")
    import time
    for i in range(30):  # 30 second timeout
        status = subprocess.run(
            [gabb_binary, "daemon", "status", "--format", "json"],
            cwd=test_dir,
            capture_output=True,
            text=True,
        )
        if status.returncode == 0:
            try:
                data = json.loads(status.stdout)
                # Stats are nested under "stats" key
                stats = data.get("stats", {})
                files_indexed = stats.get("files_indexed", 0)
                if data.get("running") and files_indexed > 0:
                    print(f"  ✓ Indexed {files_indexed} files")
                    break
            except json.JSONDecodeError:
                pass
        time.sleep(1)
    else:
        print("  ⚠ Timeout waiting for indexing")

    # Step 5: Check daemon status
    print("\n[5] Checking daemon status...")
    result = subprocess.run(
        [gabb_binary, "daemon", "status"],
        cwd=test_dir,
        capture_output=True,
        text=True,
    )
    print(f"  Status: {result.stdout.strip()}")

    # Step 6: Test symbol lookup
    print("\n[6] Testing symbol lookup...")
    result = subprocess.run(
        [gabb_binary, "symbols", "--format", "json"],
        cwd=test_dir,
        capture_output=True,
        text=True,
    )
    if result.returncode == 0:
        try:
            symbols = json.loads(result.stdout)
            print(f"  ✓ Found {len(symbols)} symbols")
            for sym in symbols[:5]:
                print(f"    - {sym.get('name', '?')} ({sym.get('kind', '?')})")
        except json.JSONDecodeError:
            print(f"  ⚠ Could not parse symbols: {result.stdout[:100]}")
    else:
        print(f"  ❌ symbols command failed: {result.stderr}")

    # Step 7: Set up Claude settings like the benchmark does
    print("\n[7] Setting up Claude workspace config...")
    claude_dir = test_dir / ".claude"
    claude_dir.mkdir(parents=True, exist_ok=True)

    settings = {
        "mcpServers": {
            "gabb": {
                "command": gabb_binary,
                "args": ["mcp-server"],
            }
        }
    }

    settings_file = claude_dir / "settings.local.json"
    settings_file.write_text(json.dumps(settings, indent=2))
    print(f"  ✓ Created: {settings_file}")
    print(f"  Content: {json.dumps(settings, indent=2)}")

    # Step 8: Copy SKILL.md
    print("\n[8] Copying SKILL.md...")
    skill_src = CONFIGS_DIR / "gabb" / "skills" / "gabb" / "SKILL.md"
    if skill_src.exists():
        skill_dst = claude_dir / "skills" / "gabb"
        skill_dst.mkdir(parents=True, exist_ok=True)
        shutil.copy(skill_src, skill_dst / "SKILL.md")
        print(f"  ✓ Copied SKILL.md to {skill_dst}")
    else:
        print(f"  ❌ SKILL.md not found at {skill_src}")

    # Step 9: Test Claude with a simple prompt to check MCP availability
    print("\n[9] Testing Claude Code with MCP server...")

    # Create MCP config file for --mcp-config flag
    mcp_config = {
        "mcpServers": {
            "gabb": {
                "command": gabb_binary,
                "args": ["mcp-server", "--workspace", str(test_dir)],
            }
        }
    }
    mcp_config_file = test_dir / "mcp_config.json"
    mcp_config_file.write_text(json.dumps(mcp_config, indent=2))
    print(f"  Created MCP config: {mcp_config_file}")
    print(f"  Content: {json.dumps(mcp_config, indent=2)}")

    print("  Running: claude -p '...' --mcp-config mcp_config.json --output-format json")

    env = os.environ.copy()
    env.update(load_env_file())

    # Allow all gabb MCP tools
    gabb_tools = [
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
    ]

    result = subprocess.run(
        [
            "claude", "-p", "List all available MCP tools. Just list their names, nothing else.",
            "--mcp-config", str(mcp_config_file),
            "--allowedTools", *gabb_tools,
            "--output-format", "json"
        ],
        cwd=test_dir,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )

    if result.returncode == 0:
        try:
            output = json.loads(result.stdout)
            answer = output.get("result", "")
            print(f"  ✓ Claude responded")
            print(f"  Response preview: {answer[:500]}...")

            # Check if gabb tools are mentioned
            if "gabb" in answer.lower():
                print("\n  ✓✓✓ SUCCESS: gabb MCP tools appear to be available!")
            else:
                print("\n  ⚠ WARNING: 'gabb' not found in response - MCP may not be configured")
        except json.JSONDecodeError:
            print(f"  ⚠ Could not parse response: {result.stdout[:200]}")
    else:
        print(f"  ❌ Claude command failed")
        print(f"    stdout: {result.stdout[:200] if result.stdout else '(empty)'}")
        print(f"    stderr: {result.stderr[:200] if result.stderr else '(empty)'}")

    # Step 10: Test Claude with a task that should use gabb
    print("\n[10] Testing Claude with a gabb-targeted prompt...")
    prompt = """Use the gabb_symbols MCP tool to find the UserService class in this codebase.

IMPORTANT: You MUST use the gabb_symbols tool, not Grep or Read.

If gabb tools are not available, say "GABB_NOT_AVAILABLE".
If gabb tools ARE available, use gabb_symbols and report what you find."""

    result = subprocess.run(
        [
            "claude", "-p", prompt,
            "--mcp-config", str(mcp_config_file),
            "--allowedTools", *gabb_tools,
            "--output-format", "json"
        ],
        cwd=test_dir,
        env=env,
        capture_output=True,
        text=True,
        timeout=120,
    )

    if result.returncode == 0:
        try:
            output = json.loads(result.stdout)
            answer = output.get("result", "")
            print(f"  Response: {answer[:800]}")

            if "GABB_NOT_AVAILABLE" in answer:
                print("\n  ❌ FAILURE: gabb MCP tools are NOT available to Claude!")
            elif "UserService" in answer and "class" in answer.lower():
                print("\n  ✓✓✓ SUCCESS: Claude found UserService using gabb!")
            else:
                print("\n  ⚠ Unclear result - check response above")
        except json.JSONDecodeError:
            print(f"  ⚠ Could not parse: {result.stdout[:200]}")
    else:
        print(f"  ❌ Failed: {result.stderr[:200] if result.stderr else result.stdout[:200]}")

    # Cleanup
    print("\n[11] Cleaning up...")
    subprocess.run([gabb_binary, "daemon", "stop"], cwd=test_dir, capture_output=True)
    shutil.rmtree(test_dir, ignore_errors=True)
    print("  ✓ Cleaned up test directory")

    print("\n" + "=" * 60)
    print("Validation complete")
    print("=" * 60)
    return 0


if __name__ == "__main__":
    sys.exit(main())
