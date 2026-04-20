use pinocchio::{account_info::AccountInfo, entrypoint, pubkey::Pubkey, ProgramResult};
use prop_amm_submission_sdk::{set_return_data_bytes, set_return_data_u64, set_storage};

const NAME: &str = "My Strategy";
const MODEL_USED: &str = "GPT-5.3-Codex"; // Use "None" for fully human-written submissions.
const FEE_DENOMINATOR: u128 = 10000;
const BASE_FEE: u128 = 70;
const EXTRA_PER_10PCT: u128 = 30;
const MAX_EXTRA: u128 = 150;
const STORAGE_SIZE: usize = 1024;

fn fee_num_for(reserve_x: u128, reserve_y: u128, step: u64) -> u128 {
    let target_y = reserve_x.saturating_mul(100);
    let diff = if target_y > reserve_y { target_y - reserve_y } else { reserve_y - target_y };
    let imb_permille = if reserve_y == 0 { 0 } else { diff.saturating_mul(1000) / reserve_y };
    let imb_extra = (imb_permille / 100).saturating_mul(EXTRA_PER_10PCT).min(MAX_EXTRA);
    let step_extra: u128 = if step > 2000 { 20 } else if step > 500 { 10 } else { 0 };
    let extra = (imb_extra + step_extra).min(MAX_EXTRA + 20);
    FEE_DENOMINATOR.saturating_sub(BASE_FEE).saturating_sub(extra)
}

fn read_counter(storage: &[u8]) -> u64 {
    if storage.len() < 8 {
        return 0;
    }
    u64::from_le_bytes([
        storage[0], storage[1], storage[2], storage[3],
        storage[4], storage[5], storage[6], storage[7],
    ])
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
        // tag 2 = after_swap: increment step counter in storage bytes 0..8
        2 => {
            // after_swap layout: tag(1) side(1) input(8) output(8) rx(8) ry(8) step(8) storage(1024)
            if instruction_data.len() >= 42 + STORAGE_SIZE {
                let incoming_storage = &instruction_data[42..42 + STORAGE_SIZE];
                let mut new_storage = [0u8; STORAGE_SIZE];
                new_storage.copy_from_slice(incoming_storage);
                let count = read_counter(&new_storage);
                let next = count.saturating_add(1);
                new_storage[0..8].copy_from_slice(&next.to_le_bytes());
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

    let step = read_counter(&decoded._storage);
    let k = reserve_x * reserve_y;
    let fee_num = fee_num_for(reserve_x, reserve_y, step);

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

/// After each real fill, increment step counter in storage bytes 0..8.
/// Native path: storage is the mutable current storage.
pub fn after_swap(_data: &[u8], storage: &mut [u8]) {
    if storage.len() < 8 {
        return;
    }
    let count = read_counter(storage);
    let next = count.saturating_add(1);
    storage[0..8].copy_from_slice(&next.to_le_bytes());
}
