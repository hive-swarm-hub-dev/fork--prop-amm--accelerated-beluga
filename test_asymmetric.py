import subprocess, re, statistics

CONFIGS = {
    "A": (9930, 9950),
    "B": (9950, 9930),
    "C": (9920, 9960),
    "D": (9960, 9920),
}

BASE = open("/Users/tianhaowu/hive/prop-amm/strategy.rs").read()

TEMPLATE = BASE.replace(
    "const FEE_NUMERATOR: u128 = 9940;",
    "const FEE_NUM_BUY: u128 = {buy};\nconst FEE_NUM_SELL: u128 = {sell};"
).replace(
    "let net_y = input_amount * FEE_NUMERATOR / FEE_DENOMINATOR;",
    "let net_y = input_amount * FEE_NUM_BUY / FEE_DENOMINATOR;"
).replace(
    "let net_x = input_amount * FEE_NUMERATOR / FEE_DENOMINATOR;",
    "let net_x = input_amount * FEE_NUM_SELL / FEE_DENOMINATOR;"
)

def make_src(buy, sell):
    src = BASE.replace(
        "const FEE_NUMERATOR: u128 = 9940;",
        f"const FEE_NUM_BUY: u128 = {buy};\nconst FEE_NUM_SELL: u128 = {sell};"
    ).replace(
        "let net_y = input_amount * FEE_NUMERATOR / FEE_DENOMINATOR;",
        "let net_y = input_amount * FEE_NUM_BUY / FEE_DENOMINATOR;"
    ).replace(
        "let net_x = input_amount * FEE_NUMERATOR / FEE_DENOMINATOR;",
        "let net_x = input_amount * FEE_NUM_SELL / FEE_DENOMINATOR;"
    )
    return src

def run_eval():
    r = subprocess.run(
        ["bash", "eval/eval.sh"],
        cwd="/Users/tianhaowu/hive/prop-amm",
        capture_output=True, text=True, timeout=300
    )
    combined = r.stdout + r.stderr
    if "validate failed" in combined:
        return None, combined
    edges = re.findall(r"Avg edge:\s*([\d.]+)", combined)
    if edges:
        return float(edges[-1]), combined
    return None, combined

results = {}
for name, (buy, sell) in CONFIGS.items():
    src = make_src(buy, sell)
    open("/Users/tianhaowu/hive/prop-amm/strategy.rs", "w").write(src)
    runs = []
    for i in range(3):
        val, log = run_eval()
        if val is None:
            print(f"Config {name} run {i+1}: FAILED\n{log[:500]}")
            runs.append(None)
        else:
            print(f"Config {name} run {i+1}: {val}")
            runs.append(val)
    results[name] = runs

print("\n=== RESULTS ===")
print(f"{'Config':8} {'r1':8} {'r2':8} {'r3':8} {'mean':8}")
best_name, best_mean = None, 0
for name, runs in results.items():
    valid = [r for r in runs if r is not None]
    mean = statistics.mean(valid) if valid else 0
    r1, r2, r3 = [(str(r) if r else "ERR") for r in runs]
    print(f"{name:8} {r1:8} {r2:8} {r3:8} {mean:.2f}")
    if mean > best_mean:
        best_mean, best_name = mean, name

print(f"\nBaseline: 408")
print(f"Best: Config {best_name} mean={best_mean:.2f}")

if best_mean > 412:
    buy, sell = CONFIGS[best_name]
    src = TEMPLATE.format(buy=buy, sell=sell)
    open("/Users/tianhaowu/hive/prop-amm/strategy.rs", "w").write(src)
    print(f"strategy.rs set to Config {best_name} (FEE_NUM_BUY={buy}, FEE_NUM_SELL={sell})")
else:
    restore = BASE  # original 9940 symmetric
    open("/Users/tianhaowu/hive/prop-amm/strategy.rs", "w").write(restore)
    print("No config beat 412 — restored symmetric 60bps (FEE_NUMERATOR=9940)")
