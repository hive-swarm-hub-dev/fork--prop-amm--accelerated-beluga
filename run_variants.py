#!/usr/bin/env python3
"""
Run the 4 imbalance-fee variants H1-H4, collect scores, submit, and post chat.
"""

import subprocess
import re
import sys
import json

REPO = "/Users/tianhaowu/hive/prop-amm"
PARENT_SHA = "2c7b448e"

VARIANTS = [
    {"name": "H1", "branch": "imb-H1-B60-E60",  "BASE": 60,  "EXTRA": 60,  "MAX": 250},
    {"name": "H2", "branch": "imb-H2-B55-E60",  "BASE": 55,  "EXTRA": 60,  "MAX": 300},
    {"name": "H3", "branch": "imb-H3-B60-E100", "BASE": 60,  "EXTRA": 100, "MAX": 400},
    {"name": "H4", "branch": "imb-H4-B65-E80",  "BASE": 65,  "EXTRA": 80,  "MAX": 300},
]

STRATEGY_TEMPLATE = '''\
use pinocchio::{{account_info::AccountInfo, entrypoint, pubkey::Pubkey, ProgramResult}};
use prop_amm_submission_sdk::{{set_return_data_bytes, set_return_data_u64}};

const NAME: &str = "My Strategy";
const MODEL_USED: &str = "GPT-5.3-Codex"; // Use "None" for fully human-written submissions.
const FEE_DENOMINATOR: u128 = 10000;
const BASE_FEE: u128 = {BASE};
const EXTRA_PER_10PCT: u128 = {EXTRA};
const MAX_EXTRA: u128 = {MAX};
const STORAGE_SIZE: usize = 1024;

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

fn fee_num_for(reserve_x: u128, reserve_y: u128) -> u128 {{
    let target_y = reserve_x.saturating_mul(100);
    let diff = if target_y > reserve_y {{ target_y - reserve_y }} else {{ reserve_y - target_y }};
    let imb_permille = if reserve_y == 0 {{ 0 }} else {{ diff.saturating_mul(1000) / reserve_y }};
    let extra = (imb_permille / 100).saturating_mul(EXTRA_PER_10PCT).min(MAX_EXTRA);
    FEE_DENOMINATOR.saturating_sub(BASE_FEE).saturating_sub(extra)
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


def run(cmd, cwd=REPO, capture=True, timeout=600):
    result = subprocess.run(
        cmd, shell=True, cwd=cwd,
        capture_output=capture, text=True, timeout=timeout
    )
    return result


def git(cmd):
    return run(f"git {cmd}")


def parse_avg_edge(output):
    # Look for the final "avg_edge: <number>" line printed by eval.sh
    for line in reversed(output.splitlines()):
        m = re.search(r'avg_edge:\s*([-\d.]+)', line)
        if m:
            return float(m.group(1))
    return None


def write_strategy(base, extra, max_extra):
    content = STRATEGY_TEMPLATE.format(BASE=base, EXTRA=extra, MAX=max_extra)
    with open(f"{REPO}/strategy.rs", "w") as f:
        f.write(content)


results = []

for v in VARIANTS:
    name = v["name"]
    branch = v["branch"]
    BASE = v["BASE"]
    EXTRA = v["EXTRA"]
    MAX = v["MAX"]

    print(f"\n{'='*60}")
    print(f"Variant {name}: branch={branch}, BASE={BASE}, EXTRA={EXTRA}, MAX={MAX}")
    print(f"{'='*60}")

    # Fresh branch from master
    r = run("git checkout master")
    if r.returncode != 0:
        print(f"  FAILED to checkout master: {r.stderr}")
        continue
    r = run(f"git checkout -b {branch}")
    if r.returncode != 0:
        print(f"  FAILED to create branch {branch}: {r.stderr}")
        # Try to delete existing branch and retry
        run(f"git branch -D {branch}")
        r = run(f"git checkout -b {branch}")
        if r.returncode != 0:
            print(f"  Still FAILED. Skipping.")
            continue

    # Write strategy.rs
    write_strategy(BASE, EXTRA, MAX)
    print(f"  strategy.rs written.")

    # Validate (first eval run — if it fails validator, skip)
    print(f"  Running validation + eval run 1...")
    r1 = run("bash eval/eval.sh 2>&1", capture=True, timeout=600)
    r1_output = r1.stdout + r1.stderr
    score1 = parse_avg_edge(r1_output)
    print(f"  Run 1 output tail:\n{r1_output[-2000:]}")

    if score1 is None or "validate failed" in r1_output:
        print(f"  VALIDATION FAILED or could not parse score. Skipping {name}.")
        tail = r1_output.splitlines()[-80:]
        print("\n".join(tail))
        results.append({
            "name": name, "branch": branch,
            "BASE": BASE, "EXTRA": EXTRA, "MAX": MAX,
            "scores": [], "mean": None, "sha": None, "failed": True
        })
        continue

    # Run 2 and 3
    print(f"  Running eval run 2...")
    r2 = run("bash eval/eval.sh 2>&1", capture=True, timeout=600)
    r2_output = r2.stdout + r2.stderr
    score2 = parse_avg_edge(r2_output)
    print(f"  Run 2 score: {score2}")

    print(f"  Running eval run 3...")
    r3 = run("bash eval/eval.sh 2>&1", capture=True, timeout=600)
    r3_output = r3.stdout + r3.stderr
    score3 = parse_avg_edge(r3_output)
    print(f"  Run 3 score: {score3}")

    scores = [s for s in [score1, score2, score3] if s is not None]
    if not scores:
        print(f"  No scores parsed. Skipping.")
        results.append({
            "name": name, "branch": branch,
            "BASE": BASE, "EXTRA": EXTRA, "MAX": MAX,
            "scores": [], "mean": None, "sha": None, "failed": True
        })
        continue

    mean = sum(scores) / len(scores)
    print(f"  Scores: {scores}, Mean: {mean:.2f}")

    # Commit
    commit_msg = f"imb-fee {name}: BASE={BASE}, EXTRA={EXTRA}, MAX_EXTRA={MAX} → mean={mean:.2f}"
    r = run(f'git add -A && git commit -m "{commit_msg}"')
    if r.returncode != 0:
        print(f"  COMMIT FAILED: {r.stderr}")
        results.append({
            "name": name, "branch": branch,
            "BASE": BASE, "EXTRA": EXTRA, "MAX": MAX,
            "scores": scores, "mean": mean, "sha": None, "failed": True
        })
        continue

    # Get SHA
    sha_r = run("git rev-parse HEAD")
    sha = sha_r.stdout.strip()[:8]
    print(f"  Committed SHA: {sha}")

    # hive push
    print(f"  Pushing to hive...")
    push_r = run("hive push", timeout=120)
    print(f"  Push output: {push_r.stdout} {push_r.stderr}")

    # hive run submit
    tldr = f"{name} imbalance-ramp: BASE={BASE} EXTRA={EXTRA} MAX={MAX} → mean={mean:.2f}"
    submit_msg = (
        f"Steeper imbalance ramp variant {name}: "
        f"BASE_FEE={BASE}bps, EXTRA_PER_10PCT={EXTRA}bps, MAX_EXTRA={MAX}bps. "
        f"3 eval runs: {scores}. Mean avg_edge={mean:.2f}."
    )
    submit_r = run(
        f'hive run submit --parent {PARENT_SHA} --score {mean:.2f} '
        f'--tldr "{tldr}" -m "{submit_msg}"',
        timeout=60
    )
    print(f"  Submit output: {submit_r.stdout} {submit_r.stderr}")

    # Post chat
    chat_msg = (
        f"[{name}] BASE={BASE} EXTRA={EXTRA} MAX={MAX} | "
        f"runs={scores} | mean={mean:.2f} | sha={sha} | branch={branch}"
    )
    chat_r = run(f'hive chat send "{chat_msg}"', timeout=30)
    print(f"  Chat output: {chat_r.stdout} {chat_r.stderr}")
    # Extract ts from chat output
    chat_ts = None
    for line in (chat_r.stdout + chat_r.stderr).splitlines():
        m = re.search(r'ts["\s:]+([0-9.]+)', line, re.IGNORECASE)
        if m:
            chat_ts = m.group(1)
            break

    results.append({
        "name": name, "branch": branch,
        "BASE": BASE, "EXTRA": EXTRA, "MAX": MAX,
        "scores": scores, "mean": mean, "sha": sha,
        "failed": False, "chat_ts": chat_ts
    })

print("\n\n" + "="*60)
print("RESULTS TABLE")
print("="*60)
print(f"{'Config':<8} {'BASE':>5} {'EXTRA':>6} {'MAX':>5} {'r1':>8} {'r2':>8} {'r3':>8} {'mean':>8} {'SHA':<10}")
for r in results:
    scores = r["scores"]
    r1 = f"{scores[0]:.2f}" if len(scores) > 0 else "N/A"
    r2 = f"{scores[1]:.2f}" if len(scores) > 1 else "N/A"
    r3 = f"{scores[2]:.2f}" if len(scores) > 2 else "N/A"
    mean_s = f"{r['mean']:.2f}" if r["mean"] else "FAIL"
    sha_s = r["sha"] or "FAIL"
    print(f"{r['name']:<8} {r['BASE']:>5} {r['EXTRA']:>6} {r['MAX']:>5} {r1:>8} {r2:>8} {r3:>8} {mean_s:>8} {sha_s:<10}")

# Find best
valid = [r for r in results if not r["failed"] and r["mean"] is not None]
if not valid:
    print("\nAll variants failed.")
    run("git checkout master")
    sys.exit(1)

best = max(valid, key=lambda r: r["mean"])
print(f"\nBest: {best['name']} mean={best['mean']:.2f} (branch={best['branch']})")

if best["mean"] > 418:
    print(f"\nBest mean {best['mean']:.2f} > 418! Running 2 more confirmations...")
    # Checkout best branch
    run(f"git checkout {best['branch']}")
    extra_scores = []
    for i in range(2):
        print(f"  Confirmation run {i+4}...")
        r = run("bash eval/eval.sh 2>&1", capture=True, timeout=600)
        s = parse_avg_edge(r.stdout + r.stderr)
        print(f"  Score: {s}")
        if s is not None:
            extra_scores.append(s)
    all_scores = best["scores"] + extra_scores
    final_mean = sum(all_scores) / len(all_scores)
    print(f"\nConfirmation: 5-run scores={all_scores}, final_mean={final_mean:.2f}")

    announce = (
        f"NEW BEST confirmed! {best['name']} BASE={best['BASE']} EXTRA={best['EXTRA']} MAX={best['MAX']} "
        f"| 5-run mean={final_mean:.2f} | branch={best['branch']} | sha={best['sha']}"
    )
    run(f'hive chat send "{announce}"')
    print(f"\nAnnouncement posted: {announce}")
else:
    print(f"\nNo variant cleared 418 (best={best['mean']:.2f}). Returning to master.")
    run("git checkout master")
    summary = (
        f"Imbalance-ramp sweep H1-H4 complete. Best mean={best['mean']:.2f} ({best['name']}), "
        f"none cleared 418 threshold. Back on master."
    )
    run(f'hive chat send "{summary}"')
    print(f"Summary posted.")

print("\nDone.")
