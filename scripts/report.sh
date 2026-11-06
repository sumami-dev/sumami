#!/usr/bin/env bash
# Run everything and write one report.
#
# The report says what was measured, what came out, and what was **not** measured
# — so it can be read as a record rather than as a claim.
#
#   ./scripts/report.sh                 # run and write benchmarks/report-<date>.md
#   ./scripts/report.sh --print         # ... and print it
#
# It is not a daily job: the daily budget run in CI asserts the memory ceilings,
# and this one is for when somebody wants the whole picture.
set -uo pipefail

cd "$(dirname "$0")/.."
ROOT=$(pwd)
DATE=$(date -u +%Y-%m-%d)
OUT="$ROOT/benchmarks/report-$DATE.md"

mkdir -p "$ROOT/benchmarks"
: > "$OUT"

say() { printf '%s\n' "$*" >> "$OUT"; }

# --------------------------------------------------------------------------------------
# The tests
# --------------------------------------------------------------------------------------

say "# The test report — $DATE"
say ""
say "One run of everything: the tests, the lint, the memory budget and the benchmarks."
say ""

say "## The tests"
say ""
say '```'
say "\$ cargo test --workspace"
TESTS=$(cargo test --workspace 2>&1)
echo "$TESTS" | grep -E "^(test result|error)" >> "$OUT"
PASSED=$(echo "$TESTS" | grep -oE "^test result: ok\. [0-9]+" | grep -oE "[0-9]+" | paste -sd+ - | bc)
FAILED=$(echo "$TESTS" | grep -cE "^test result: FAILED")
say '```'
say ""
say "**$PASSED passed, $FAILED files failed.**"
say ""

say '```'
say "\$ cargo clippy --workspace --all-targets -- -D warnings"
CLIPPY=$(cargo clippy --workspace --all-targets -- -D warnings 2>&1)
CLIPPY_STATUS=$?
say '```'
say ""
if [ "$CLIPPY_STATUS" -eq 0 ]; then
    say "**Clippy: clean.**"
else
    say "**Clippy: FAILED.**"
    say '```'
    echo "$CLIPPY" | tail -30 >> "$OUT"
    say '```'
fi
say ""

if command -v deno > /dev/null; then
    say "## The tracker's tests"
    say ""
    say '```'
    say "\$ cd tracker && deno test --allow-env src/"
    TRACKER=$(cd "$ROOT/tracker" && deno test --allow-env src/ 2>&1)
    TRACKER_STATUS=$?
    echo "$TRACKER" | grep -E "^(ok|FAILED|error|passed)" | tail -20 >> "$OUT"
    say '```'
    say ""
    if [ "$TRACKER_STATUS" -eq 0 ]; then
        say "**The tracker: green.**"
    else
        say "**The tracker: FAILED.**"
    fi
    say ""
fi

# --------------------------------------------------------------------------------------
# The memory budget
# --------------------------------------------------------------------------------------

say "## The memory budget"
say ""
say "Two tiers, the whole process's resident set against the ceiling the product"
say "promises. The **exact** peak is read from \`/proc/self/status\` (VmHWM) — that is"
say "Linux only; elsewhere the number is the highest value a sampler saw, which is"
say "a lower bound."
say ""
say '```'
say "\$ cargo run --release --example memory_budget -p sumimi-service -- --engine 128MB"
cargo build --release -p sumimi-service --example memory_budget > /dev/null 2>&1
"$ROOT/target/release/examples/memory_budget" --engine 128MB 2>&1 | tail -2 >> "$OUT"
say "\$ cargo run --release --example memory_budget -p sumimi-service -- --engine 256MB"
"$ROOT/target/release/examples/memory_budget" --engine 256MB 2>&1 | tail -2 >> "$OUT"
say '```'
say ""

# --------------------------------------------------------------------------------------
# The benchmarks
# --------------------------------------------------------------------------------------

say "## The benchmarks"
say ""

say "### The two engines, the same data"
say ""
say '```'
say "\$ cargo run --release --example engine_compare -p sumimi-service"
cargo run --release --example engine_compare -p sumimi-service 2>&1 | tail -16 >> "$OUT"
say '```'
say ""

say "### What a page view costs"
say ""
say '```'
say "\$ cargo run --release --example row_cost -p sumimi-service"
cargo run --release --example row_cost -p sumimi-service 2>&1 | tail -32 >> "$OUT"
say '```'
say ""

say "### What the write path costs"
say ""
say '```'
say "\$ cargo run --release --example write_probe -p sumimi-service"
cargo run --release --example write_probe -p sumimi-service 2>&1 | tail -24 >> "$OUT"
say '```'
say ""

say "## 这次没有量的"
say ""
say "- **精确的峰值 RSS** —— 要 Linux（\`/proc/self/status\` 的 VmHWM）✓；"
say "  macOS 上是采样值 ✓，是下界 ✓；"
say "- **浸泡测试** —— 10 GB 的那一趟要几十分钟 ✓，不在这里 ✓；"
say "- **两千万行的规模测试** ✓ —— 见 [sumimi-pro#1](https://github.com/sumimi-dev/sumimi-pro/issues/1) ✓。"
say ""

echo "wrote $OUT"
if [ "${1:-}" = "--print" ]; then
    cat "$OUT"
fi

# --------------------------------------------------------------------------------------
# The report goes into an issue
# --------------------------------------------------------------------------------------
#
# A number nobody can find is a number nobody trusts. The report is posted, so
# it can be read, compared with the last one, and argued with.

if [ "${1:-}" != "--no-post" ]; then
    URL=$(gh issue create --repo sumimi-dev/sumimi \
        --title "The test report — $DATE" \
        --body-file "$OUT" 2>&1 | tail -1)
    echo "posted: $URL"
fi
