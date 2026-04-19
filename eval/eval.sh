#!/usr/bin/env bash
# Run the prop-amm benchmark on strategy.rs.
#
# Two-step eval to match the leaderboard's scoring guarantees:
#
#   1. `prop-amm validate strategy.rs` enforces the same gates the server uses:
#      monotonicity, concavity, randomized reserve/storage checks, the 100k-CU
#      compute-unit budget, and — critically — native/BPF output parity. The
#      validator compiles your program both ways and rejects the submission if
#      the two runtimes diverge.
#
#   2. `prop-amm run strategy.rs` (native) runs 1000 simulations from a fresh
#      seed-start. Because step 1 has already proven that native and BPF
#      produce byte-identical compute_swap outputs for this strategy, the
#      native score is the score the leaderboard's BPF eval would compute on
#      the same seeds. Native is ~100x faster than BPF on Apple Silicon (no
#      JIT), so we use native for the actual sim loop.
#
# A strategy that fails step 1 is rejected with score 0 — the same outcome as a
# server-side rejection. Don't ship it.
set -euo pipefail
cd "$(dirname "$0")/.."

VENDOR="vendor/prop-amm-challenge"
CLI="$VENDOR/target/release/prop-amm"
STRATEGY="$(pwd)/strategy.rs"

if [[ ! -x "$CLI" ]]; then
  echo "prop-amm CLI missing; run \`bash prepare.sh\` first" >&2
  exit 1
fi
if [[ ! -f "$STRATEGY" ]]; then
  echo "strategy.rs not found at $STRATEGY" >&2
  exit 1
fi
if ! command -v cargo-build-sbf >/dev/null 2>&1; then
  echo "cargo-build-sbf missing; run \`bash prepare.sh\` first" >&2
  exit 1
fi

# Sample one seed-start at runtime — the public server uses a different seed
# schedule, so re-rolling locally discourages overfitting to seed 0..999.
SEED_START="$(python3 -c 'import secrets; print(secrets.randbelow(2**31))')"
SIMULATIONS=1000
WORKERS=4

echo "# seed-start: ${SEED_START}"

# Run from inside the vendored repo so the CLI's hardcoded
# `../../../crates/submission-sdk` path resolves correctly. The `.build/runs/`
# scratch dir lives inside vendor (gitignored).
echo "# validating strategy.rs (monotonicity, concavity, native/BPF parity, CU budget)"
if ! ( cd "$VENDOR" && "./target/release/prop-amm" validate "$STRATEGY" ); then
  echo "validate failed — submission would be rejected by the server" >&2
  echo "---"
  echo "avg_edge:         0.000000"
  echo "correct:          0"
  echo "total:            ${SIMULATIONS}"
  exit 0
fi

RUN_OUT="$(mktemp)"
trap 'rm -f "$RUN_OUT"' EXIT

( cd "$VENDOR" && "./target/release/prop-amm" run "$STRATEGY" \
    --simulations "$SIMULATIONS" \
    --workers "$WORKERS" \
    --seed-start "$SEED_START" ) | tee "$RUN_OUT"

# CLI prints e.g. "  Avg edge:    -3.41" — extract the number.
AVG_EDGE="$(awk -F: '/Avg edge:/ {gsub(/[ \t]+/,"",$2); print $2; exit}' "$RUN_OUT")"
if [[ -z "${AVG_EDGE:-}" ]]; then
  echo "failed to parse Avg edge from CLI output" >&2
  exit 1
fi

# Every requested simulation runs to completion in this engine — no failure
# bookkeeping is exposed by the CLI, so correct == total.
SIM_COUNT="$(awk '/Simulations:/ {print $2; exit}' "$RUN_OUT")"

echo "---"
printf "avg_edge:         %.6f\n" "$AVG_EDGE"
echo "correct:          ${SIM_COUNT}"
echo "total:            ${SIM_COUNT}"
