#!/bin/bash
#
# Run a single astropy SWE-bench task 20 times to gather statistically
# significant benchmark data comparing control vs gabb conditions.
#
# Usage: ./run_astropy_benchmark.sh [task_id]
#
# Default task: astropy__astropy-12907 (single file: astropy/modeling/separable.py)
#

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BENCHMARK_DIR="$SCRIPT_DIR/claude-code"

# Task to run (can be overridden via first argument)
TASK_ID="${1:-astropy__astropy-12907}"
RUNS=20

echo "=============================================="
echo "Gabb Benchmark Runner"
echo "=============================================="
echo "Task:    $TASK_ID"
echo "Runs:    $RUNS per condition (control + gabb)"
echo "Project: $PROJECT_ROOT"
echo ""

# Step 1: Build release binary
echo "[1/2] Building release binary..."
cd "$PROJECT_ROOT"
cargo build --release --quiet

GABB_BINARY="$PROJECT_ROOT/target/release/gabb"
if [[ ! -x "$GABB_BINARY" ]]; then
    echo "ERROR: Release binary not found at $GABB_BINARY"
    exit 1
fi

VERSION=$("$GABB_BINARY" --version)
echo "      Built: $VERSION"
echo "      Path:  $GABB_BINARY"
echo ""

# Step 2: Run benchmark
echo "[2/2] Running benchmark..."
echo "      This will take a while (~5-10 min per run × $RUNS runs × 2 conditions)"
echo ""

cd "$BENCHMARK_DIR"
python3 run.py \
    --swe-bench "$TASK_ID" \
    --gabb-binary "$GABB_BINARY" \
    --runs "$RUNS" \
    --verbose

echo ""
echo "=============================================="
echo "Benchmark complete!"
echo "Results saved to: $BENCHMARK_DIR/results/"
echo "=============================================="
