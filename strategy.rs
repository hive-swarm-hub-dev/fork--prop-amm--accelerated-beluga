use pinocchio::{account_info::AccountInfo, entrypoint, pubkey::Pubkey, ProgramResult};
use prop_amm_submission_sdk::{set_return_data_bytes, set_return_data_u64, set_storage};

const NAME: &str = "My Strategy";
const MODEL_USED: &str = "GPT-5.3-Codex"; // Use "None" for fully human-written submissions.
const FEE_DENOMINATOR: u128 = 10000;
const BASE_FEE: u128 = 70;
const EXTRA_PER_10PCT: u128 = 30;
const MAX_EXTRA: u128 = 150;
const STORAGE_SIZE: usize = 1024;

// price_ema_q32 stored at storage[0..8] as u64 little-endian (Q32.32 fixed point)
// actual_price = raw / 2^32

fn fee_num_for(reserve_x: u128, reserve_y: u128, price_ema_q32: u64) -> u128 {
    let target_ratio_q32: u128 = if price_ema_q32 == 0 {
        100u128 << 32 // default to 100 Q32.32
    } else {
        price_ema_q32 as u128
    };
    // target_y = reserve_x * target_ratio_q32 / 2^32
    let target_y = reserve_x.saturating_mul(target_ratio_q32) >> 32;
    let diff = if target_y > reserve_y { target_y - reserve_y } else { reserve_y - target_y };
    let imb_permille = if reserve_y == 0 { 0 } else { diff.saturating_mul(1000) / reserve_y };
    let extra = (imb_permille / 100).saturating_mul(EXTRA_PER_10PCT).min(MAX_EXTRA);
    FEE_DENOMINATOR.saturating_sub(BASE_FEE).saturating_sub(extra)
}

#[derive(wincode::SchemaRead)]
struct ComputeSwapInstruction {
    side: u8,
    input_amount: u64,
    reserve_x: u64,
    reserve_y: u64,
    storage: [u8; STORAGE_SIZE],
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
        // tag 2 = after_swap: update price EMA in storage
        2 => {
            // afterSwap payload offsets:
            // 0: tag, 1: side, 2-9: input_amount, 10-17: output_amount,
            // 18-25: reserve_x, 26-33: reserve_y, 34-41: step, 42-1065: storage
            if instruction_data.len() >= 42 + STORAGE_SIZE {
                let mut storage_buf = [0u8; STORAGE_SIZE];
                storage_buf.copy_from_slice(&instruction_data[42..42 + STORAGE_SIZE]);
                after_swap(instruction_data, &mut storage_buf);
                let _ = set_storage(&storage_buf);
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

// after_swap signature fn(&[u8], &mut [u8]) required by native shim.
// data: full afterSwap instruction payload
// storage: current storage buffer (pre-populated); update in-place
pub fn after_swap(data: &[u8], storage: &mut [u8]) {
    // afterSwap payload:
    // offset 0: tag (1 byte)
    // offset 1: side (1 byte)
    // offset 2: input_amount (8 bytes)
    // offset 10: output_amount (8 bytes)
    // offset 18: reserve_x (8 bytes)
    // offset 26: reserve_y (8 bytes)
    // offset 34: step (8 bytes)
    // offset 42: storage (1024 bytes)
    if data.len() < 26 + 8 {
        return;
    }

    let reserve_x = u64::from_le_bytes(data[18..26].try_into().unwrap_or([0u8; 8]));
    let reserve_y = u64::from_le_bytes(data[26..34].try_into().unwrap_or([0u8; 8]));

    if reserve_x == 0 || storage.len() < 8 {
        return;
    }

    // Read old EMA from storage (passed in by native, copied from data[42..] in BPF)
    let old_ema = u64::from_le_bytes(storage[0..8].try_into().unwrap_or([0u8; 8]));

    // Compute pool-implied price in Q32.32: price = reserve_y / reserve_x
    let instant_price_q32_u128 = ((reserve_y as u128) << 32) / (reserve_x as u128);
    let instant_price_q32 = instant_price_q32_u128.min(u64::MAX as u128) as u64;

    // EMA update with alpha = 1/16
    // Bootstrap: if old_ema == 0, seed with instant price directly to avoid low initial value
    let new_ema_u128 = if old_ema == 0 {
        instant_price_q32 as u128
    } else {
        ((old_ema as u128) * 15 + (instant_price_q32 as u128)) / 16
    };
    let new_ema = new_ema_u128.min(u64::MAX as u128) as u64;

    storage[0..8].copy_from_slice(&new_ema.to_le_bytes());
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

    let price_ema_q32 = u64::from_le_bytes(decoded.storage[0..8].try_into().unwrap_or([0u8; 8]));

    let k = reserve_x * reserve_y;
    let fee_num = fee_num_for(reserve_x, reserve_y, price_ema_q32);

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
