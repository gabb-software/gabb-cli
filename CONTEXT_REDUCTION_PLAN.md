# Context Reduction Plan

## Problem Statement

Benchmark results show a **179% increase in token usage** when using gabb tools compared to control (Grep/Read/Glob):

| Metric | Control | Gabb | Diff |
|--------|---------|------|------|
| Total Tokens | 60,545 | 169,138 | +179% |
| Tool Calls | 23.5 | 5.5 | -76% |
| Turns | 2.8 | 6.5 | +132% |

**Key insight:** Fewer tool calls but massively more tokens means each gabb tool response is ~20x larger than equivalent Grep/Read output.

---

## Checklist of Potential Fixes

### Category 1: Tool Output Defaults

#### 1.1 Change `gabb_definition` default to `include_source=false`
- [ ] **Status:** Not started
- **Location:** `src/mcp.rs:1252`
- **Current:** `unwrap_or(true)` - includes full source by default
- **Change to:** `unwrap_or(false)` - return location only by default
- **Risk:** Low - users can still request source explicitly
- **Expected impact:** HIGH - definition is commonly called

```rust
// Before
let include_source = args.get("include_source").and_then(|v| v.as_bool()).unwrap_or(true);

// After
let include_source = args.get("include_source").and_then(|v| v.as_bool()).unwrap_or(false);
```

#### 1.2 Change `gabb_symbol` default to `include_source=false`
- [ ] **Status:** Not started
- **Location:** `src/mcp.rs` in `tool_symbol` function
- **Current behavior:** Check what the default is
- **Expected impact:** MEDIUM

#### 1.3 Change `gabb_symbols` default to `include_source=false`
- [ ] **Status:** Not started
- **Location:** `src/mcp.rs` in `tool_symbols` function
- **Expected impact:** MEDIUM-HIGH (commonly used for broad searches)

---

### Category 2: Output Format Optimization

#### 2.1 Add source line limit parameter
- [ ] **Status:** Not started
- **Description:** Add `max_source_lines` parameter that truncates source after N lines
- **Default:** 20-30 lines
- **Implementation:** Modify `extract_source()` to accept limit

#### 2.2 Add compact output mode
- [ ] **Status:** Not started
- **Description:** New parameter `output_mode: "compact" | "full"`
- **Compact returns:** `kind name file:line:col` (one line per symbol)
- **Full returns:** Current verbose format with source

#### 2.3 Truncate long function bodies with ellipsis
- [ ] **Status:** Not started
- **Description:** For functions >30 lines, show first 10 + `...` + last 5
- **Location:** `format_symbol()` in `src/mcp.rs:2331`

#### 2.4 Remove redundant metadata from output
- [ ] **Status:** Not started
- **Review:** Check if visibility, container, context are always needed
- **Consider:** Only include non-default values

---

### Category 3: SKILL.md and Documentation

#### 3.1 Remove `include_source=true` from default examples
- [ ] **Status:** Not started
- **Location:** `.claude/skills/gabb/SKILL.md:48, 57`
- **Change:** Show location-first workflow, source as optional

#### 3.2 Add explicit guidance on when NOT to use include_source
- [ ] **Status:** Not started
- **Add:** Clear warning that include_source should only be used for single, specific symbols

#### 3.3 Promote two-step pattern more strongly
- [ ] **Status:** Not started
- **Pattern:**
  1. `gabb_structure` or `gabb_symbol` (no source) to find location
  2. `Read file offset=X limit=Y` to get specific code
- **Rationale:** Read is more token-efficient for viewing code

#### 3.4 Update CLAUDE.md guidance
- [ ] **Status:** Not started
- **Location:** `CLAUDE.md` tool selection table
- **Change:** Emphasize location-first, source-second approach

---

### Category 4: Structural Changes

#### 4.1 Investigate why turns increased (6.5 vs 2.8)
- [ ] **Status:** Not started
- **Hypothesis:** Claude calls fewer tools per turn with gabb, requiring more back-and-forth
- **Investigation:** Review benchmark transcripts to understand turn patterns
- **Potential fix:** Better tool descriptions that encourage batching

#### 4.2 Add tool response size limits
- [ ] **Status:** Not started
- **Description:** Hard limit on tool response size (e.g., 2000 chars)
- **Overflow behavior:** Truncate with "... (truncated, use Read for full source)"

#### 4.3 Return file:line:col format consistently
- [ ] **Status:** Not started
- **Current:** Various formats across tools
- **Standardize:** `path/to/file.rs:123:5` for all location outputs
- **Benefit:** Smaller output, Claude can use Read if needed

---

### Category 5: Measurement & Testing

#### 5.1 Add output size metrics to benchmarks
- [ ] **Status:** Not started
- **Measure:** Bytes per tool call for each tool type
- **Compare:** Gabb tool output size vs Grep/Read output size

#### 5.2 Create token usage regression tests
- [ ] **Status:** Not started
- **Test:** Fixed queries should produce output under size threshold
- **Example:** `gabb_structure src/mcp.rs` should be <2KB

#### 5.3 A/B test individual changes
- [ ] **Status:** Not started
- **Method:** Run benchmark with single change at a time
- **Track:** Which changes have biggest impact

#### 5.4 Capture and analyze benchmark transcripts
- [ ] **Status:** Not started
- **Goal:** Understand exactly what Claude is asking for and receiving
- **Look for:** Unnecessary source inclusion, repeated queries

---

### Category 6: Alternative Approaches

#### 6.1 Lazy source loading via follow-up tool
- [ ] **Status:** Not started
- **Concept:** `gabb_source symbol_id=X` separate tool to fetch source
- **Benefit:** Source only fetched when explicitly needed

#### 6.2 Reference-based output
- [ ] **Status:** Not started
- **Concept:** Return "See src/mcp.rs:100-150" instead of inline source
- **Benefit:** Claude uses Read tool for actual code viewing

#### 6.3 Hierarchical detail levels
- [ ] **Status:** Not started
- **Levels:**
  - L0: Name and location only
  - L1: + signature/type info
  - L2: + docstring/comments
  - L3: + full source
- **Default to L0 or L1**

---

## Priority Order

Based on expected impact and implementation effort:

| Priority | Item | Expected Impact | Effort |
|----------|------|-----------------|--------|
| 1 | 1.1 Change definition default | HIGH | LOW |
| 2 | 3.1 Update SKILL.md examples | HIGH | LOW |
| 3 | 1.3 Change symbols default | MEDIUM-HIGH | LOW |
| 4 | 2.1 Add source line limit | MEDIUM | MEDIUM |
| 5 | 5.4 Analyze transcripts | DIAGNOSTIC | MEDIUM |
| 6 | 4.1 Investigate turn increase | DIAGNOSTIC | MEDIUM |
| 7 | 2.3 Truncate long functions | MEDIUM | MEDIUM |
| 8 | 6.2 Reference-based output | HIGH | HIGH |

---

## Success Criteria

- [ ] Token usage within 50% of control (currently +179%)
- [ ] Maintain task success rate (currently 100%)
- [ ] Maintain or improve time savings (currently -48%)

---

## Notes

- The benchmark task is `astropy__astropy-14995` - should analyze what this task requires
- Control uses: Glob (1.4), Grep (6.0), Read (6.4), Bash (8.8), Task (1.0)
- Gabb uses: structure (1.6), symbol (1.0), symbols (0.5), plus minimal Glob/Grep/Read
- The fact that gabb solves it 48% faster with 76% fewer tool calls is good - just need to reduce output verbosity
