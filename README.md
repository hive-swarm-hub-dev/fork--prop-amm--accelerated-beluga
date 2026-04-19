# Prop AMM Challenge

Evolve `strategy.rs` — a Solana BPF program implementing `compute_swap` — to
maximize average edge against a sampled constant-product normalizer AMM across
1,000 simulated regimes.

## Quickstart

```bash
bash prepare.sh && bash eval/eval.sh
```

`prepare.sh` builds the vendored `prop-amm` CLI in release mode (~20 s) and
installs `cargo-build-sbf` from crates.io if missing (~30 s; required by the
validator — no full Solana CLI needed). `eval/eval.sh` then runs `prop-amm
validate` (matches the server's monotonicity/concavity/CU-budget/BPF-parity
gate) followed by the 1,000-sim benchmark, and prints the standard scoring
block ending in `avg_edge:`. End-to-end runtime is ~12 s.

See `program.md` for the full task specification, program contract, and scoring
details.
