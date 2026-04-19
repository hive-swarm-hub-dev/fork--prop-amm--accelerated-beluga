use pinocchio::{account_info::AccountInfo, entrypoint, pubkey::Pubkey, ProgramResult};
use prop_amm_submission_sdk::{set_return_data_bytes, set_return_data_u64, set_storage};

const NAME: &str = "My Strategy";
const MODEL_USED: &str = "GPT-5.3-Codex"; // Use "None" for fully human-written submissions.
const FEE_DENOMINATOR: u128 = 10000;
const STORAGE_SIZE: usize = 1024;

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
        // tag 2 = after_swap: increment step counter in storage
        2 => {
            if instruction_data.len() >= 42 + 8 {
                // after_swap layout: tag(1) + side(1) + input(8) + output(8) + rx(8) + ry(8) + step(8) + storage(1024)
                // storage starts at offset 42
                let stor_offset = 42;
                let old_step = if instruction_data.len() >= stor_offset + 8 {
                    u64::from_le_bytes(
                        instruction_data[stor_offset..stor_offset + 8]
                            .try_into()
                            .unwrap_or([0u8; 8]),
                    )
                } else {
                    0
                };
                let new_step = old_step.saturating_add(1);
                let mut new_storage = [0u8; STORAGE_SIZE];
                let copy_len = if instruction_data.len() >= stor_offset + STORAGE_SIZE {
                    STORAGE_SIZE
                } else {
                    instruction_data.len().saturating_sub(stor_offset)
                };
                new_storage[..copy_len].copy_from_slice(&instruction_data[stor_offset..stor_offset + copy_len]);
                new_storage[0..8].copy_from_slice(&new_step.to_le_bytes());
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

fn fee_num_for_step(step: u64) -> u128 {
    if step < 50 {
        FEE_DENOMINATOR - 40  // 40 bps
    } else if step < 500 {
        FEE_DENOMINATOR - 60  // 60 bps
    } else {
        FEE_DENOMINATOR - 70  // 70 bps
    }
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

    let step = u64::from_le_bytes(decoded.storage[0..8].try_into().unwrap_or([0u8; 8]));
    let fee_num = fee_num_for_step(step);
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

/// Native after_swap mirror for BPF/native parity.
/// Increments step counter stored in bytes 0..8 of storage.
pub fn after_swap(_data: &[u8], storage: &mut [u8]) {
    if storage.len() >= 8 {
        let old_step = u64::from_le_bytes(storage[0..8].try_into().unwrap_or([0u8; 8]));
        let new_step = old_step.saturating_add(1);
        storage[0..8].copy_from_slice(&new_step.to_le_bytes());
    }
}
