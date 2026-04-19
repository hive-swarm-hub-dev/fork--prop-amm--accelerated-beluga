#!/usr/bin/env python3
"""Grid search over imbalance-defensive fee parameters."""
import subprocess
import re
import sys
import os

REPO = "/Users/tianhaowu/hive/prop-amm"
HIVE_SERVER = "https://hive-frontend-staging-production.up.railway.app/"

STRATEGY_TEMPLATE = '''\
use pinocchio::{{account_info::AccountInfo, entrypoint, pubkey::Pubkey, ProgramResult}};
use prop_amm_submission_sdk::{{set_return_data_bytes, set_return_data_u64}};

const NAME: &str = "My Strategy";
const MODEL_USED: &str = "GPT-5.3-Codex"; // Use "None" for fully human-written submissions.
const FEE_DENOMINATOR: u128 = 10000;
const BASE_FEE: u128 = {base};
const EXTRA_PER_10PCT: u128 = {extra};
const MAX_EXTRA: u128 = 150;
const STORAGE_SIZE: usize = 1024;

fn fee_num_for(reserve_x: u128, reserve_y: u128) -> u128 {{
    let target_y = reserve_x.saturating_mul(100);
    let diff = if target_y > reserve_y {{ target_y - reserve_y }} else {{ reserve_y - target_y }};
    let imb_permille = if reserve_y == 0 {{ 0 }} else {{ diff.saturating_mul(1000) / reserve_y }};
    let extra = (imb_permille / 100).saturating_mul(EXTRA_PER_10PCT).min(MAX_EXTRA);
    FEE_DENOMINATOR.saturating_sub(BASE_FEE).saturating_sub(extra)
}}

#[derive(wincode::SchemaRead)]
struct ComputeSwapInstruction {{
    side: u8,
    input_amount: u64,
    reserve_x: u64,
    reserve_y: u64,
    _storage: [u8; STORAGE_SIZE],
}}

#[cfg(not(feature = "no-entrypoint"))]
entrypoint!(process_instruction);

pub fn process_instruction(
    _program_id: &Pubkey,
    _accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {{
    if instruction_data.is_empty() {{
        return Ok(());
    }}

    match instruction_data[0] {{
        // tag 0 or 1 = compute_swap (side)
        0 | 1 => {{
            let output = compute_swap(instruction_data);
            set_return_data_u64(output);
        }}
        // tag 2 = after_swap (no-op for starter)
        2 => {{
            // No storage updates needed for basic CFMM
        }}
        // tag 3 = get_name (for leaderboard display)
        3 => set_return_data_bytes(NAME.as_bytes()),
        // tag 4 = get_model_used (for metadata display)
        4 => set_return_data_bytes(get_model_used().as_bytes()),
        _ => {{}}
    }}

    Ok(())
}}

pub fn get_model_used() -> &\'static str {{
    MODEL_USED
}}

pub fn compute_swap(data: &[u8]) -> u64 {{
    let decoded: ComputeSwapInstruction = match wincode::deserialize(data) {{
        Ok(decoded) => decoded,
        Err(_) => return 0,
    }};

    let side = decoded.side;
    let input_amount = decoded.input_amount as u128;
    let reserve_x = decoded.reserve_x as u128;
    let reserve_y = decoded.reserve_y as u128;

    if reserve_x == 0 || reserve_y == 0 {{
        return 0;
    }}

    let k = reserve_x * reserve_y;
    let fee_num = fee_num_for(reserve_x, reserve_y);

    match side {{
        0 => {{
            let net_y = input_amount * fee_num / FEE_DENOMINATOR;
            let new_ry = reserve_y + net_y;
            let k_div = (k + new_ry - 1) / new_ry;
            reserve_x.saturating_sub(k_div) as u64
        }}
        1 => {{
            let net_x = input_amount * fee_num / FEE_DENOMINATOR;
            let new_rx = reserve_x + net_x;
            let k_div = (k + new_rx - 1) / new_rx;
            reserve_y.saturating_sub(k_div) as u64
        }}
        _ => 0,
    }}
}}
'''

CONFIGS = [
    ("G1", 50, 30),
    ("G2", 55, 25),
    ("G3", 65, 15),
    ("G4", 70, 30),
]

def run(cmd, cwd=REPO, env=None, capture=False, timeout=600):
    full_env = os.environ.copy()
    if env:
        full_env.update(env)
    print(f"$ {cmd}", flush=True)
    result = subprocess.run(
        cmd, shell=True, cwd=cwd, env=full_env,
        capture_output=capture, text=True, timeout=timeout
    )
    if capture:
        return result.stdout + result.stderr
    return result.returncode

def run_eval():
    """Run eval once, return avg_edge float."""
    out = run("bash eval/eval.sh 2>&1", capture=True, timeout=600)
    print(out, flush=True)
    m = re.search(r'avg_edge:\s*([-\d.]+)', out)
    if m:
        return float(m.group(1))
    # Check if validate failed
    if 'validate failed' in out or 'avg_edge:         0' in out:
        return 0.0
    raise RuntimeError(f"Could not parse avg_edge from output:\n{out}")

def git_sha():
    import subprocess
    r = subprocess.run("git rev-parse HEAD", shell=True, cwd=REPO, capture_output=True, text=True)
    return r.stdout.strip()

results = []

for label, base, extra in CONFIGS:
    branch = f"grid-B{base}-E{extra}"
    print(f"\n{'='*60}", flush=True)
    print(f"CONFIG {label}: BASE={base}, EXTRA={extra}, branch={branch}", flush=True)
    print('='*60, flush=True)

    # Ensure clean master checkout
    run("git checkout master")
    run(f"git branch -D {branch} 2>/dev/null || true")
    run(f"git checkout -b {branch}")

    # Write strategy.rs
    strategy = STRATEGY_TEMPLATE.format(base=base, extra=extra)
    with open(f"{REPO}/strategy.rs", "w") as f:
        f.write(strategy)
    print(f"Written strategy.rs with BASE={base}, EXTRA={extra}", flush=True)

    # Run 3 times
    scores = []
    for run_num in range(1, 4):
        print(f"\n--- Run {run_num}/3 ---", flush=True)
        try:
            score = run_eval()
            scores.append(score)
            print(f"Run {run_num} score: {score}", flush=True)
        except Exception as e:
            print(f"Run {run_num} ERROR: {e}", flush=True)
            scores.append(0.0)

    mean = sum(scores) / len(scores) if scores else 0.0
    print(f"\n{label} scores: {scores}, mean: {mean:.2f}", flush=True)

    # Commit
    run("git add -A")
    run(f'git commit -m "Imbalance-defensive fee BASE={base}bps EXTRA={extra}/10pct"')
    sha = git_sha()
    print(f"Committed as {sha}", flush=True)

    # Push
    run(f"hive push", env={"HIVE_SERVER": HIVE_SERVER})

    # Submit
    tldr = f"Grid {label}: BASE={base}bps EXTRA={extra}/10pct. Runs: {scores[0]:.2f}, {scores[1]:.2f}, {scores[2]:.2f}. Mean {mean:.2f}."
    desc = f"Grid search config {label}: BASE_FEE={base}bps, EXTRA_PER_10PCT={extra}bps/10pct imbalance, MAX_EXTRA=150bps. 3-run mean={mean:.2f}."
    run(
        f'hive run submit -m "{desc}" --score {mean:.2f} --parent 2c7b448e --tldr "{tldr}"',
        env={"HIVE_SERVER": HIVE_SERVER}
    )

    results.append({
        "label": label, "base": base, "extra": extra,
        "branch": branch, "scores": scores, "mean": mean, "sha": sha
    })

# Summary
print("\n\n" + "="*70, flush=True)
print("GRID SEARCH RESULTS", flush=True)
print("="*70, flush=True)
print(f"{'Config':<8} {'Base':<6} {'Extra':<7} {'Run1':<10} {'Run2':<10} {'Run3':<10} {'Mean':<10} {'SHA'}", flush=True)
print("-"*80, flush=True)
for r in results:
    s = r['scores']
    print(f"{r['label']:<8} {r['base']:<6} {r['extra']:<7} {s[0]:<10.2f} {s[1]:<10.2f} {s[2]:<10.2f} {r['mean']:<10.2f} {r['sha'][:8]}", flush=True)

# Find best
best = max(results, key=lambda r: r['mean'])
print(f"\nBest: {best['label']} (BASE={best['base']}, EXTRA={best['extra']}) mean={best['mean']:.2f}", flush=True)

THRESHOLD = 415.0
if best['mean'] > THRESHOLD:
    print(f"\nBest mean {best['mean']:.2f} > {THRESHOLD} — merging winning branch into master", flush=True)
    run("git checkout master")
    run(f"git merge --ff-only {best['branch']}")
    msg = (f"Grid search complete. Best config: {best['label']} "
           f"(BASE={best['base']}, EXTRA={best['extra']}), mean={best['mean']:.2f}, "
           f"beats master 408.91 well past noise. New master = {best['sha'][:8]}.")
else:
    print(f"\nNo config cleared {THRESHOLD} — returning to master (2c7b448e)", flush=True)
    run("git checkout master")
    msg = (f"Grid search complete (4 configs). Best: {best['label']} "
           f"BASE={best['base']} EXTRA={best['extra']} mean={best['mean']:.2f}. "
           f"None beat 415 threshold. Master unchanged at 2c7b448e (408.91).")

# Post chat message
print(f"\nPosting chat: {msg}", flush=True)
run(f'hive chat send "{msg}"', env={"HIVE_SERVER": HIVE_SERVER})
print("\nDONE.", flush=True)

# Print final results for deliverable
print("\n\n=== FINAL DELIVERABLE ===", flush=True)
for r in results:
    s = r['scores']
    print(f"{r['label']} | {s[0]:.2f} | {s[1]:.2f} | {s[2]:.2f} | {r['mean']:.2f} | SHA: {r['sha'][:8]}", flush=True)
print(f"Best mean: {best['mean']:.2f} ({best['label']})", flush=True)
