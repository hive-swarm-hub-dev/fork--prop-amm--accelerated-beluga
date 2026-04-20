use pinocchio::{account_info::AccountInfo, entrypoint, pubkey::Pubkey, ProgramResult};
use prop_amm_submission_sdk::{set_return_data_bytes, set_return_data_u64, set_storage};

const NAME: &str = "My Strategy";
const MODEL_USED: &str = "GPT-5.3-Codex"; // Use "None" for fully human-written submissions.
const FEE_DENOMINATOR: u128 = 10000;
const STORAGE_SIZE: usize = 1024;

const MIN_FEE_BPS: u128 = 35;
const MAX_FEE_BPS: u128 = 250;
const BOOTSTRAP_FEE_BPS: u128 = 70;
const BOOTSTRAP_MIN_SAMPLES: u32 = 24;
const KAPPA_NUM: u128 = 1;
const KAPPA_DEN: u128 = 1;

// Q32.32 log2 of a positive u64. Returns log2(x) * 2^32 as i64.
// Accuracy: linear interp of fractional part from bits below the MSB.
// For x=0 returns 0 (caller must avoid).
fn ilog2_q32(x: u64) -> i64 {
    if x == 0 {
        return 0;
    }
    let lz = x.leading_zeros();
    let n = 63u32.wrapping_sub(lz) as i64; // floor(log2(x)), 0..=63
    // Normalize: shift x so MSB is at bit 63.
    let normalized = x << lz;
    // Fractional bits: bits 62..31 of `normalized` give a Q0.32 fractional part.
    // normalized = 2^63 + frac*2^31 approximately; extract top 32 bits after the leading 1.
    let frac_q32 = ((normalized >> 31) & 0xFFFF_FFFF) as i64;
    (n << 32) | frac_q32
}

fn read_i64_le(b: &[u8]) -> i64 {
    i64::from_le_bytes(b[..8].try_into().unwrap_or([0u8; 8]))
}

fn read_u64_le(b: &[u8]) -> u64 {
    u64::from_le_bytes(b[..8].try_into().unwrap_or([0u8; 8]))
}

fn read_u32_le(b: &[u8]) -> u32 {
    u32::from_le_bytes(b[..4].try_into().unwrap_or([0u8; 4]))
}

fn write_i64_le(b: &mut [u8], v: i64) {
    b[..8].copy_from_slice(&v.to_le_bytes());
}

fn write_u64_le(b: &mut [u8], v: u64) {
    b[..8].copy_from_slice(&v.to_le_bytes());
}

fn write_u32_le(b: &mut [u8], v: u32) {
    b[..4].copy_from_slice(&v.to_le_bytes());
}

fn compute_fee_bps(storage: &[u8]) -> u128 {
    if storage.len() < 20 {
        return BOOTSTRAP_FEE_BPS;
    }
    let n = read_u32_le(&storage[16..20]);
    if n < BOOTSTRAP_MIN_SAMPLES {
        return BOOTSTRAP_FEE_BPS;
    }
    let ewma_q32 = read_u64_le(&storage[8..16]) as u128;
    // sigma in log2 units per step, Q32.32.
    // Convert to bps: multiply by 10000, shift by 32.
    let sigma_bps = (ewma_q32 * 10_000) >> 32;
    let fee_bps = (sigma_bps * KAPPA_NUM) / KAPPA_DEN;
    fee_bps.clamp(MIN_FEE_BPS, MAX_FEE_BPS)
}

// Core afterSwap logic: reads reserves+step from `data`, mutates `storage`.
// afterSwap payload offsets:
//   0: tag (1), 1: side (1), 2: input_amount (8), 10: output_amount (8),
//   18: reserve_x (8), 26: reserve_y (8), 34: step (8), 42: storage (1024)
fn update_state(data: &[u8], storage: &mut [u8]) {
    if data.len() < 42 || storage.len() < 24 {
        return;
    }

    let reserve_x = read_u64_le(&data[18..26]);
    let reserve_y = read_u64_le(&data[26..34]);
    let step = read_u64_le(&data[34..42]);

    // Compute current log ratio: log2(ry/rx) = log2(ry) - log2(rx)
    let lr = if reserve_x == 0 || reserve_y == 0 {
        0i64
    } else {
        ilog2_q32(reserve_y) - ilog2_q32(reserve_x)
    };

    // Read prior state
    let last_lr = read_i64_le(&storage[0..8]);
    let ewma = read_u64_le(&storage[8..16]);
    let n = read_u32_le(&storage[16..20]);

    // Compute per-step delta: raw |Delta log2|
    let dlog_abs: u64 = if n == 0 {
        0
    } else {
        lr.wrapping_sub(last_lr).unsigned_abs()
    };

    // EWMA: new = old - (old >> 6) + (sample >> 6)  // alpha = 1/64
    let ewma_new = ewma
        .saturating_sub(ewma >> 6)
        .saturating_add(dlog_abs >> 6);

    // Write back state
    write_i64_le(&mut storage[0..8], lr);
    write_u64_le(&mut storage[8..16], ewma_new);
    write_u32_le(&mut storage[16..20], n.saturating_add(1));
    write_u32_le(&mut storage[20..24], step as u32);
}

// Native after_swap hook: called by the native shim with fn(&[u8], &mut [u8]) signature.
// The simulator passes a fresh mutable storage slice; mutations persist automatically.
pub fn after_swap(data: &[u8], storage: &mut [u8]) {
    update_state(data, storage);
}

#[derive(wincode::SchemaRead)]
struct ComputeSwapInstruction {
    side: u8,
    input_amount: u64,
    reserve_x: u64,
    reserve_y: u64,
    _storage: [u8; STORAGE_SIZE],
}

#[cfg(not(feature = "no-entrypoint"))]
entrypoint!(process_instruction);

pub fn process_instruction(
    _program_id: &Pubkey,
    _accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    if instruction_data.is_empty() {
        return Ok(());
    }

    match instruction_data[0] {
        // tag 0 or 1 = compute_swap (side)
        0 | 1 => {
            let output = compute_swap(instruction_data);
            set_return_data_u64(output);
        }
        // tag 2 = after_swap (BPF path: parse storage from payload, update, persist)
        2 => {
            // In BPF, storage is embedded in instruction_data at offset 42.
            // Parse it out, update, then persist via set_storage.
            if instruction_data.len() >= 42 + STORAGE_SIZE {
                let mut buf = [0u8; STORAGE_SIZE];
                buf.copy_from_slice(&instruction_data[42..42 + STORAGE_SIZE]);
                update_state(instruction_data, &mut buf);
                let _ = set_storage(&buf);
            }
        }
        // tag 3 = get_name (for leaderboard display)
        3 => set_return_data_bytes(NAME.as_bytes()),
        // tag 4 = get_model_used (for metadata display)
        4 => set_return_data_bytes(get_model_used().as_bytes()),
        _ => {}
    }

    Ok(())
}

pub fn get_model_used() -> &'static str {
    MODEL_USED
}

pub fn compute_swap(data: &[u8]) -> u64 {
    let decoded: ComputeSwapInstruction = match wincode::deserialize(data) {
        Ok(decoded) => decoded,
        Err(_) => return 0,
    };

    let side = decoded.side;
    let input_amount = decoded.input_amount as u128;
    let reserve_x = decoded.reserve_x as u128;
    let reserve_y = decoded.reserve_y as u128;

    if reserve_x == 0 || reserve_y == 0 {
        return 0;
    }

    let fee_bps = compute_fee_bps(&decoded._storage);
    let fee_num = FEE_DENOMINATOR - fee_bps;

    let k = reserve_x * reserve_y;

    match side {
        0 => {
            let net_y = input_amount * fee_num / FEE_DENOMINATOR;
            let new_ry = reserve_y + net_y;
            let k_div = (k + new_ry - 1) / new_ry;
            reserve_x.saturating_sub(k_div) as u64
        }
        1 => {
            let net_x = input_amount * fee_num / FEE_DENOMINATOR;
            let new_rx = reserve_x + net_x;
            let k_div = (k + new_rx - 1) / new_rx;
            reserve_y.saturating_sub(k_div) as u64
        }
        _ => 0,
    }
}
