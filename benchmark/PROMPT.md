# Role
You are a Senior SDET specializing in AI Agent evaluation.

# Objective
Build a modular, extensible benchmarking suite in Python to prove that `gabb-cli` (a semantic indexing tool) reduces token usage and improves navigation speed compared to standard tools (`grep`/`ripgrep`/`find`/`read`).

# Context
- **Target Tool:** `gabb-cli` (A binary executable built from the rust project in `..`). It supports Python, TS, Rust.
- **Dataset:** `princeton-nlp/SWE-bench_Verified` (HuggingFace).
- **Hypothesis:** Agents using `gabb` will find the relevant files and source code for an issue significantly faster and with less token overhead than agents using `grep`, `find` and `read`.

# The Plan
Please implement the system following these three phases. Start with Phase 1.

## Phase 1: The "Walking Skeleton" (Retrieval Verification)
**Goal:** Run ONE specific SWE-bench task end-to-end and measure if the agent can *locate* the file to edit.

### 1. Architecture (`benchmark/`)
Create a modular design that separates the Environment, Agent, and Task logic.

*   **`core/env.py`**:
    *   `BenchmarkEnv`: A wrapper around the `docker` SDK.
    *   **Crucial:** It must mount the local `./gabb` binary into the container at `/usr/local/bin/gabb` and ensure it is executable.
    *   *Optimization:* For Phase 1, do not use full SWE-bench images. Use a lightweight `python:3.11` image and simply `git clone` the repo. We are testing retrieval, not execution.

*   **`core/agent.py`**:
    *   `BaseAgent`: Abstract class managing the Anthropic client loop.
    *   `ControlAgent`: Equipped with `grep`, `find_file`, `read_file` (with line limits).
    *   `GabbAgent`: Equipped with the gabb MCP server and skill.
    *   **System Prompt:** "You are a code navigation assistant. Given an issue, identify the full path of the file(s) that need to be modified. Output 'FINAL_ANSWER: <filepath>' when sure."

*   **`core/dataset.py`**:
    *   Use `datasets` to load `SWE-bench_Verified`.
    *   Implement `parse_gold_files(patch_text)`: Extract the filenames modified in the "patch" field. This is our ground truth.

### 2. The Runner (`run.py`)
*   Hardcode one task (e.g., `scikit-learn__scikit-learn-10297`) for the initial test.
*   Run both agents (Control vs Gabb).
*   Compare `FINAL_ANSWER` vs `Gold Files`.
*   Log: `tokens_input`, `tokens_output`, `turns`, `time_seconds`, `success`.

---

## Phase 2: The Retrieval Suite (Concurrency)
**Goal:** Run on 20-50 tasks concurrently to generate statistically significant data.

*   **Concurrency:** Update `run.py` to use `asyncio` to run multiple Docker containers in parallel.
*   **Reporting:** Output a `results.csv` and a summary table comparing:
    *   Recall Rate (Did it find the right file?)
    *   Avg Context Cost (Tokens)
    *   Avg Speed (Time/Turns)

---

## Phase 3: Full SWE-bench (Future Extensibility)
*   Design the `BenchmarkEnv` so we can later swap the "Lightweight Python Image" for the official "SWE-bench Docker Image".
*   Design the `Agent` class so we can later swap the "Retrieval Prompt" for a "Code Fixing Prompt".

# Implementation Instructions
1.  Start by creating the directory structure.
2.  Implement `core/dataset.py` first to ensure we can parse the gold standard files from the patches.
3.  Implement `core/env.py` and verify you can mount the binary.
4.  Create the Phase 1 runner to test on `scikit-learn__scikit-learn-10297`.