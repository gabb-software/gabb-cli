# Gabb Benchmark Telemetry

Analyze Claude Code conversation transcripts to identify gabb optimization opportunities.

This is **Phase 1** of the [benchmark telemetry system](https://github.com/gabb-software/gabb-cli/issues/97), providing:

- **Transcript parser** - Extracts tool calls from Claude Code JSON format
- **Bash command classifier** - Decomposes commands into type, pattern, target
- **Token estimator** - Counts tokens using tiktoken
- **Report generator** - Outputs analysis in text, JSON, or rich format

## Installation

```bash
cd benchmark/telemetry
pip install -e .

# Or with development dependencies
pip install -e ".[dev]"
```

## Usage

```bash
# Analyze a single transcript
gabb-benchmark analyze conversation.json

# Output as JSON for further processing
gabb-benchmark analyze conversation.json --format json > report.json

# Analyze multiple transcripts with aggregate summary
gabb-benchmark analyze *.json --summary

# Plain text output (no colors)
gabb-benchmark analyze conversation.json --format text
```

## Transcript Format

The parser accepts Claude Code conversation transcripts in Messages API format:

```json
{
  "messages": [
    {"role": "user", "content": "Fix the auth bug"},
    {"role": "assistant", "content": [
      {"type": "text", "text": "I'll look into this..."},
      {"type": "tool_use", "id": "toolu_01", "name": "Bash", "input": {"command": "grep -rn 'handleAuth' src/"}}
    ]},
    {"role": "user", "content": [
      {"type": "tool_result", "tool_use_id": "toolu_01", "content": "src/auth.ts:42: export function handleAuth..."}
    ]}
  ]
}
```

## Sample Output

```
══════════════════════════════════════════════════════════════════════
                    Gabb Benchmark Report
══════════════════════════════════════════════════════════════════════

Task: Find where the handleAuth function is defined

TOKEN SUMMARY
──────────────────────────────────────────────────────────────────────
  Total turns:          3
  Total tool calls:     2
  Input tokens:         1,234
  Output tokens:        456
  File content tokens:  890 (52%)

TOOL DISTRIBUTION
──────────────────────────────────────────────────────────────────────
  Bash                                1 calls         234 tokens
  Read                                1 calls         656 tokens

  Bash command breakdown:
    grep                              1

══════════════════════════════════════════════════════════════════════
```

## Running Tests

```bash
cd benchmark/telemetry
pip install -e ".[dev]"
pytest
```

## Next Steps (Future Phases)

- **Phase 2**: Opportunity detection rules (grep → gabb_usages, etc.)
- **Phase 3**: Token savings estimation
- **Phase 4**: Recommendation engine
- **Phase 5**: Benchmark comparison mode

See [issue #97](https://github.com/gabb-software/gabb-cli/issues/97) for the full roadmap.
