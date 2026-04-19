import subprocess
import re
import os

STRATEGY_PATH = "/Users/tianhaowu/hive/prop-amm/strategy.rs"
WORK_DIR = "/Users/tianhaowu/hive/prop-amm"
LOG_PATH = os.path.join(WORK_DIR, "run.log")

FEE_DENOMINATOR = 10000
FEES = [
    (20, 9980),
    (40, 9960),
    (60, 9940),
    (80, 9920),
]

def set_fees(numerator, denominator):
    with open(STRATEGY_PATH, "r") as f:
        content = f.read()
    content = re.sub(r'const FEE_NUMERATOR: u128 = \d+;', f'const FEE_NUMERATOR: u128 = {numerator};', content)
    content = re.sub(r'const FEE_DENOMINATOR: u128 = \d+;', f'const FEE_DENOMINATOR: u128 = {denominator};', content)
    with open(STRATEGY_PATH, "w") as f:
        f.write(content)

def run_eval():
    result = subprocess.run(
        "bash eval/eval.sh > run.log 2>&1",
        shell=True, cwd=WORK_DIR
    )
    with open(LOG_PATH, "r") as f:
        log = f.read()
    matches = re.findall(r'Avg edge:\s*([\d.]+)', log)
    if matches:
        return float(matches[-1])
    return None

results = []
for bps, numerator in FEES:
    set_fees(numerator, FEE_DENOMINATOR)
    print(f"\nRunning {bps} bps (NUM={numerator}, DEN={FEE_DENOMINATOR}) - Run 1...")
    r1 = run_eval()
    print(f"  Run 1: {r1}")
    print(f"Running {bps} bps - Run 2...")
    r2 = run_eval()
    print(f"  Run 2: {r2}")
    mean = None
    if r1 is not None and r2 is not None:
        mean = (r1 + r2) / 2
    results.append((bps, numerator, r1, r2, mean))

print("\n\n--- RESULTS TABLE ---")
print(f"{'fee_bps':>8} | {'run1':>8} | {'run2':>8} | {'mean':>8}")
print("-" * 42)
for bps, num, r1, r2, mean in results:
    r1s = f"{r1:.2f}" if r1 is not None else "N/A"
    r2s = f"{r2:.2f}" if r2 is not None else "N/A"
    ms = f"{mean:.2f}" if mean is not None else "N/A"
    print(f"{bps:>8} | {r1s:>8} | {r2s:>8} | {ms:>8}")

# Find best
valid = [(bps, num, mean) for bps, num, _, _, mean in results if mean is not None]
if valid:
    best_bps, best_num, best_mean = max(valid, key=lambda x: x[2])
    print(f"\nBest: {best_bps} bps (NUM={best_num}), mean={best_mean:.2f}")
    set_fees(best_num, FEE_DENOMINATOR)
    print(f"Restored strategy.rs to FEE_NUMERATOR={best_num}, FEE_DENOMINATOR={FEE_DENOMINATOR}")
else:
    print("No valid results — restoring to original 9980/10000")
    set_fees(9980, FEE_DENOMINATOR)
