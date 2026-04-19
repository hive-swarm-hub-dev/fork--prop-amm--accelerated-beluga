# Prop AMM Challenge

Evolve `strategy.rs` — a single Solana BPF program implementing `compute_swap` —
to maximize **average edge** in a 1,000-simulation benchmark against a
constant-product normalizer AMM.

## Setup

1. **Read the in-scope files**:
   - `strategy.rs` — the Rust source you modify. Must define `NAME`, `MODEL_USED`,
     `process_instruction`, and `compute_swap`.
   - `eval/eval.sh` — runs the benchmark via the vendored `prop-amm` CLI. Do not modify.
   - `prepare.sh` — builds the CLI from `vendor/prop-amm-challenge`. Do not modify.
   - `vendor/prop-amm-challenge/` — pinned simulator source. Do not modify.
2. **Run prepare**: `bash prepare.sh` builds the release CLI
   (`cargo build --release -p prop-amm`, ~20 s on a clean cache) and installs
   `cargo-build-sbf` from crates.io (~30 s) if it isn't already on PATH.
   `cargo-build-sbf` is required by the validator (step 1 of the eval) — no
   full Solana CLI install is needed.
3. **Verify the CLI**: confirm `vendor/prop-amm-challenge/target/release/prop-amm`
   exists and `command -v cargo-build-sbf` resolves.
4. **Initialize results.tsv**: create `results.tsv` with just the header row.
5. **Run baseline**: `bash eval/eval.sh` to establish the starting `avg_edge`.

## The benchmark

Each simulation is 10,000 steps of a stochastic market. A geometric-Brownian-motion
fair price drives an arbitrageur (golden-section search) and Poisson-arrival retail
flow that an order router splits between your AMM and a sampled normalizer
(constant-product, fee `~U{30,80} bps`, liquidity multiplier `~U[0.4, 2.0]`). Both
pools start at 100 X / 10,000 Y. Your edge per trade is computed against the fair
price at trade time: retail fills produce positive edge from the spread, arbitrage
fills produce negative edge from adverse selection. The benchmark aggregates 1,000
sampled regimes into a single mean.

## Experimentation

**What you CAN do:**
- Modify `strategy.rs`. Reshape the price curve, add dynamic spreads, track volatility
  via the optional `afterSwap` hook (instruction tag `2`) and the 1,024-byte storage
  buffer, add inventory skew — anything that respects the contract below.
- Add helper modules inline within `strategy.rs` (no extra files; no `mod foo;`).

**What you CANNOT do (server will reject):**
- Modify `eval/`, `prepare.sh`, or `vendor/`.
- Use `unsafe` Rust, `include!`, `include_str!`, `include_bytes!`, `env!`,
  `option_env!`, `extern crate`, or external `mod foo;` files. The submission
  must be one self-contained `lib.rs`.
- Depend on anything outside `pinocchio` and `wincode` — the only crates the
  server permits.
- Break **monotonicity** (larger input → larger output) or **concavity**
  (diminishing returns).
- Exceed **100k compute units** per `compute_swap` invocation.

**The local eval enforces every numeric constraint** (monotonicity, concavity,
100k-CU cap, native/BPF parity) via `prop-amm validate strategy.rs` before
running the sim loop. A strategy that fails validate scores `avg_edge: 0`
locally — the same rejection it would get from the server. The remaining
restrictions (`unsafe`, `include!`/`include_str!`/`include_bytes!`, `env!`,
`option_env!`, `extern crate`, external `mod foo;`, non-allowed crates) are
syntactic source-level rules the server enforces at submission time and the
local CLI does not currently re-check; honour them.

**The goal: maximize `avg_edge`** — mean per-simulation edge across 1,000 sampled
regimes. Higher is better. The starter (constant-product, 500 bps fee) scores
around `+210` per local run: the wide spread keeps arbitrageurs away while still
capturing rare retail flow. The fee schedule is wildly inefficient — there is
ample headroom for tighter, more adaptive pricing.

**Simplicity criterion**: All else being equal, a simpler `strategy.rs` that holds
its score across seed regimes is preferred over a complex one that scores marginally
higher on a single seed.

## Program contract

`process_instruction` must dispatch on the first byte of `instruction_data`:

| Tag | Meaning                          | Required action                                                    |
|-----|----------------------------------|--------------------------------------------------------------------|
| 0   | compute_swap (buy X, Y in)       | Decode `ComputeSwapInstruction`; return `output_x` via `set_return_data_u64` |
| 1   | compute_swap (sell X, X in)      | Decode `ComputeSwapInstruction`; return `output_y` via `set_return_data_u64` |
| 2   | afterSwap (post-fill notification) | Optional. Update storage via `set_storage` if you persist state.  |
| 3   | get_name                         | `set_return_data_bytes(NAME.as_bytes())`                           |
| 4   | get_model_used                   | `set_return_data_bytes(get_model_used().as_bytes())`               |

`afterSwap` payload (offsets in bytes):

| Offset | Size | Field         |
|--------|------|---------------|
| 0      | 1    | tag (`2`)     |
| 1      | 1    | side          |
| 2      | 8    | input_amount  |
| 10     | 8    | output_amount |
| 18     | 8    | reserve_x     |
| 26     | 8    | reserve_y     |
| 34     | 8    | step          |
| 42     | 1024 | storage       |

Storage is zero-initialised at the start of each simulation and persists across
trades within that simulation only. `afterSwap` is called only on real fills (not
during golden-section quoting).

## Evaluation

```bash
bash eval/eval.sh
```

`eval.sh` is a **two-step gate** designed to match the public leaderboard's
scoring guarantees:

1. **`prop-amm validate strategy.rs`** — enforces the same hard checks the
   server applies: monotonicity, concavity, randomized reserve/storage probes,
   the 100k-CU compute-unit cap, and **native/BPF output parity**. The validator
   compiles your strategy both ways and refuses to pass it unless every probe
   produces byte-identical outputs across runtimes. Empirically the parity
   `delta` is exactly `0.000000000` for the starter (12 sims × 2000 steps).
2. **`prop-amm run strategy.rs --simulations 1000 --workers 4 --seed-start <seed>`** —
   the actual benchmark. Because step 1 has just proven your `compute_swap`
   produces identical outputs under native and BPF, the native score equals the
   score the leaderboard's BPF eval would compute on the same seeds. Native is
   ~100× faster than the BPF interpreter on Apple Silicon (no JIT), so we use
   native for the actual sim loop.

A strategy that fails step 1 is rejected with `avg_edge: 0` — same outcome
your submission would face on the server. Don't ship it.

Total wall time: ~1 s validate + ~11 s native sim ≈ **~12 s per eval** on M3.
The seed-start is freshly sampled via `secrets.randbelow(2**31)` each run.

Output:

```
# seed-start: <int>
...
---
avg_edge:         <float, 6 decimal places>
correct:          1000
total:            1000
```

Parse the score with: `grep "^avg_edge:" run.log`

**Reproducibility note.** Because `seed-start` is re-sampled each invocation,
`avg_edge` will fluctuate run-to-run even when `strategy.rs` is unchanged.
Normalizer fee/liquidity vary widely across simulations, so the run-to-run noise
floor is non-trivial — treat tenths-of-an-edge differences as noise and optimise
for the distribution, not a lucky seed.

## Output Format

The eval script always ends with:

```
---
avg_edge:         <float, 6 decimal places>
correct:          <int, simulations completed>
total:            <int, simulations requested>
```
