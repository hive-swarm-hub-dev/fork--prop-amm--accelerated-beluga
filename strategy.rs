use pinocchio::{account_info::AccountInfo, entrypoint, pubkey::Pubkey, ProgramResult};
use prop_amm_submission_sdk::{set_return_data_bytes, set_return_data_u64, set_storage};

const NAME: &str = "My Strategy";
const MODEL_USED: &str = "GPT-5.3-Codex"; // Use "None" for fully human-written submissions.
const FEE_DENOMINATOR: u128 = 10000;
const BASE_FEE: u128 = 70;
const EXTRA_PER_10PCT: u128 = 30;
const MAX_EXTRA: u128 = 200;
const BURST_COEFF: u128 = 40;
const STORAGE_SIZE: usize = 1024;

fn fee_num_for(reserve_x: u128, reserve_y: u128, burst_level: u64) -> u128 {
    let target_y = reserve_x.saturating_mul(100);
    let diff = if target_y > reserve_y { target_y - reserve_y } else { reserve_y - target_y };
    let imb_permille = if reserve_y == 0 { 0 } else { diff.saturating_mul(1000) / reserve_y };
    let imb_extra = (imb_permille / 100).saturating_mul(EXTRA_PER_10PCT).min(MAX_EXTRA);
    let burst_extra = (burst_level as u128).min(100) * BURST_COEFF / 100;
    let total_extra = (imb_extra + burst_extra).min(MAX_EXTRA);
    FEE_DENOMINATOR.saturating_sub(BASE_FEE).saturating_sub(total_extra)
}

fn read_u64(buf: &[u8], offset: usize) -> u64 {
    if buf.len() < offset + 8 {
        return 0;
    }
    u64::from_le_bytes(buf[offset..offset + 8].try_into().unwrap_or([0u8; 8]))
}

fn write_u64(buf: &mut [u8], offset: usize, val: u64) {
    if buf.len() >= offset + 8 {
        buf[offset..offset + 8].copy_from_slice(&val.to_le_bytes());
    }
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
        // tag 2 = after_swap: update burst detector in storage bytes 0..16
        // layout: tag(1) side(1) input(8) output(8) rx(8) ry(8) step(8) storage(1024)
        2 => {
            if instruction_data.len() >= 42 + STORAGE_SIZE {
                let step = read_u64(instruction_data, 34);
                let incoming_storage = &instruction_data[42..42 + STORAGE_SIZE];
                let mut new_storage = [0u8; STORAGE_SIZE];
                new_storage.copy_from_slice(incoming_storage);

                let last_trade_step = read_u64(&new_storage, 0);
                let burst_level = read_u64(&new_storage, 8);

                let delta = step.saturating_sub(last_trade_step);
                let new_burst = if delta < 20 {
                    (burst_level + 25).min(100)
                } else {
                    burst_level / 2
                };

                write_u64(&mut new_storage, 0, step);
                write_u64(&mut new_storage, 8, new_burst);
                let _ = set_storage(&new_storage);
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

    let burst_level = read_u64(&decoded.storage, 8);
    let k = reserve_x * reserve_y;
    let fee_num = fee_num_for(reserve_x, reserve_y, burst_level);

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

/// Native path: update burst detector directly in mutable storage.
/// data layout: tag(1) side(1) input(8) output(8) rx(8) ry(8) step(8) ...
pub fn after_swap(data: &[u8], storage: &mut [u8]) {
    if data.len() < 42 || storage.len() < 16 {
        return;
    }
    let step = read_u64(data, 34);

    let last_trade_step = read_u64(storage, 0);
    let burst_level = read_u64(storage, 8);

    let delta = step.saturating_sub(last_trade_step);
    let new_burst = if delta < 20 {
        (burst_level + 25).min(100)
    } else {
        burst_level / 2
    };

    write_u64(storage, 0, step);
    write_u64(storage, 8, new_burst);
}
