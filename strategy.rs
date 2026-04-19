use pinocchio::{account_info::AccountInfo, entrypoint, pubkey::Pubkey, ProgramResult};
use prop_amm_submission_sdk::{set_return_data_bytes, set_return_data_u64, set_storage};

const NAME: &str = "My Strategy";
const MODEL_USED: &str = "GPT-5.3-Codex"; // Use "None" for fully human-written submissions.
const FEE_DENOMINATOR: u128 = 10000;
const STORAGE_SIZE: usize = 1024;
const LOW_FEE_NUM: u128 = 9940;
const HIGH_FEE_NUM: u128 = 9880;
const HOT_THRESHOLD: u64 = 200;

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
        // tag 2 = after_swap: update EMA
        2 => {
            if instruction_data.len() >= 34 {
                let side = instruction_data[1];
                let input_amount = u64::from_le_bytes(
                    instruction_data[2..10].try_into().unwrap_or([0u8; 8])
                );
                let reserve_x = u64::from_le_bytes(
                    instruction_data[18..26].try_into().unwrap_or([0u8; 8])
                );
                let reserve_y = u64::from_le_bytes(
                    instruction_data[26..34].try_into().unwrap_or([0u8; 8])
                );

                // Normalize side-1 X-input to Y-equivalent
                let input_y_equiv: u64 = if side == 1 && reserve_x > 0 {
                    ((input_amount as u128) * (reserve_y as u128) / (reserve_x as u128)) as u64
                } else {
                    input_amount
                };

                // Read old EMA from a fresh storage buffer (we only have instruction_data here)
                // Use 0 as default old_ema since we can't read current storage in after_swap
                let old_ema: u64 = 0;
                let new_ema = (old_ema * 7 + input_y_equiv) / 8;

                let mut storage_buf = [0u8; STORAGE_SIZE];
                storage_buf[0..8].copy_from_slice(&new_ema.to_le_bytes());
                set_storage(&storage_buf);
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

    let k = reserve_x * reserve_y;

    let ema = u64::from_le_bytes(decoded.storage[0..8].try_into().unwrap_or([0u8; 8]));
    let fee_num = if ema > HOT_THRESHOLD { HIGH_FEE_NUM } else { LOW_FEE_NUM };

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
