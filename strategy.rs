use pinocchio::{account_info::AccountInfo, entrypoint, pubkey::Pubkey, ProgramResult};
use prop_amm_submission_sdk::{set_return_data_bytes, set_return_data_u64, set_storage};

const NAME: &str = "My Strategy";
const MODEL_USED: &str = "GPT-5.3-Codex"; // Use "None" for fully human-written submissions.
const FEE_DENOMINATOR: u128 = 10000;
const STORAGE_SIZE: usize = 1024;

// Q32.32 log2 of a positive u64. Returns log2(x) * 2^32 as i64.
fn ilog2_q32(x: u64) -> i64 {
    if x == 0 { return 0; }
    let lz = x.leading_zeros();
    let n = 63u32.wrapping_sub(lz) as i64;
    let normalized = x << lz;
    let frac_q32 = ((normalized >> 31) & 0xFFFF_FFFF) as i64;
    (n << 32) | frac_q32
}

fn read_i64_le(b: &[u8]) -> i64 {
    i64::from_le_bytes(b[..8].try_into().unwrap_or([0u8; 8]))
}

fn read_u32_le(b: &[u8]) -> u32 {
    u32::from_le_bytes(b[..4].try_into().unwrap_or([0u8; 4]))
}

fn write_i64_le(b: &mut [u8], v: i64) {
    b[..8].copy_from_slice(&v.to_le_bytes());
}

fn write_u32_le(b: &mut [u8], v: u32) {
    b[..4].copy_from_slice(&v.to_le_bytes());
}

fn fee_num_for(reserve_x: u128, reserve_y: u128, log_p_ref: i64, n: u32, side: u8) -> u128 {
    // During bootstrap (n < 16), fall back to G4's 100:1 target.
    if n < 16 {
        let target_y = reserve_x.saturating_mul(100);
        let diff = if target_y > reserve_y { target_y - reserve_y } else { reserve_y - target_y };
        let imb_permille = if reserve_y == 0 { 0 } else { diff.saturating_mul(1000) / reserve_y };
        let extra = (imb_permille / 100).saturating_mul(30).min(150);
        return 10000u128.saturating_sub(70).saturating_sub(extra);
    }
    let rx_u64 = reserve_x.min(u64::MAX as u128) as u64;
    let ry_u64 = reserve_y.min(u64::MAX as u128) as u64;
    let lr = ilog2_q32(ry_u64) - ilog2_q32(rx_u64);
    let dev_signed: i64 = lr - log_p_ref;
    let dev_abs = dev_signed.unsigned_abs() as u128;
    let permille = dev_abs.saturating_mul(693) >> 32;
    let sym_extra = (permille / 100).saturating_mul(15).min(75);
    let adverse = (dev_signed > 0 && side == 1) || (dev_signed < 0 && side == 0);
    let asym_extra = if adverse { (permille / 100).saturating_mul(30).min(150) } else { 0 };
    let total_extra = (sym_extra + asym_extra).min(225);
    10000u128.saturating_sub(70).saturating_sub(total_extra)
}

#[derive(wincode::SchemaRead)]
struct ComputeSwapInstruction {
    side: u8,
    input_amount: u64,
    reserve_x: u64,
    reserve_y: u64,
    _storage: [u8; STORAGE_SIZE],
}

// afterSwap payload: 0=tag(1), 1=side(1), 2=input(8), 10=output(8),
// 18=reserve_x(8), 26=reserve_y(8), 34=step(8), 42=storage(1024)
fn update_state(data: &[u8], storage: &mut [u8]) {
    if data.len() < 42 || storage.len() < 16 {
        return;
    }
    let reserve_x = u64::from_le_bytes(data[18..26].try_into().unwrap_or([0u8; 8]));
    let reserve_y = u64::from_le_bytes(data[26..34].try_into().unwrap_or([0u8; 8]));
    let step = u64::from_le_bytes(data[34..42].try_into().unwrap_or([0u8; 8]));

    let lr = if reserve_x == 0 || reserve_y == 0 {
        0i64
    } else {
        ilog2_q32(reserve_y) - ilog2_q32(reserve_x)
    };

    let n = read_u32_le(&storage[8..12]);
    let last_step = read_u32_le(&storage[12..16]) as u64;

    // Bootstrap: on the very first fill, seed p_ref = lr.
    let pref = if n == 0 {
        lr
    } else {
        let cur = read_i64_le(&storage[0..8]);
        // Slow EMA α = 1/256: only advance once per step.
        if step > last_step { cur + ((lr - cur) >> 8) } else { cur }
    };

    write_i64_le(&mut storage[0..8], pref);
    write_u32_le(&mut storage[8..12], n.saturating_add(1));
    write_u32_le(&mut storage[12..16], step as u32);
}

// Native after_swap hook: detected by name, called via ffi_after_swap shim.
pub fn after_swap(data: &[u8], storage: &mut [u8]) {
    update_state(data, storage);
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
        0 | 1 => {
            let output = compute_swap(instruction_data);
            set_return_data_u64(output);
        }
        2 => {
            // BPF path: storage embedded at offset 42.
            if instruction_data.len() >= 42 + STORAGE_SIZE {
                let mut buf = [0u8; STORAGE_SIZE];
                buf.copy_from_slice(&instruction_data[42..42 + STORAGE_SIZE]);
                update_state(instruction_data, &mut buf);
                let _ = set_storage(&buf);
            }
        }
        3 => set_return_data_bytes(NAME.as_bytes()),
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

    let log_p_ref = read_i64_le(&decoded._storage[0..8]);
    let n = read_u32_le(&decoded._storage[8..12]);
    let fee_num = fee_num_for(reserve_x, reserve_y, log_p_ref, n, side);
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
