use pinocchio::{account_info::AccountInfo, entrypoint, pubkey::Pubkey, ProgramResult};
use prop_amm_submission_sdk::{set_return_data_bytes, set_return_data_u64, set_storage};

const NAME: &str = "My Strategy";
const MODEL_USED: &str = "GPT-5.3-Codex"; // Use "None" for fully human-written submissions.
const FEE_DENOMINATOR: u128 = 10000;
const BASE_FEE_BPS: u128 = 70;
const EXTRA_COEFF_BPS: u128 = 60;
const MAX_EXTRA_BPS: u128 = 250;
const SIM_LENGTH: u128 = 10_000;
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
        // tag 2 = after_swap: mirror engine step into storage
        // afterSwap payload offsets:
        // 0: tag, 1: side, 2-9: input_amount, 10-17: output_amount,
        // 18-25: reserve_x, 26-33: reserve_y, 34-41: step, 42-1065: storage
        2 => {
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

// after_swap: mirror the engine-provided step (data[34..42]) into storage[0..8]
pub fn after_swap(data: &[u8], storage: &mut [u8]) {
    if data.len() < 42 || storage.len() < 8 {
        return;
    }
    // Copy engine step verbatim — authoritative, no drift vs BPF/native
    storage[0..8].copy_from_slice(&data[34..42]);
}

fn fee_num_for(side: u8, rx: u128, ry: u128, step: u128) -> u128 {
    let target_y = rx.saturating_mul(100);
    let (pool_long_x, diff) = if target_y > ry {
        (true, target_y - ry)
    } else {
        (false, ry - target_y)
    };
    let imb_permille = if ry == 0 { 0 } else { diff.saturating_mul(1000) / ry };

    // Time-decay factor in [0, 1000]: 1000 at t=0, 0 at t=T
    let t_remaining = SIM_LENGTH.saturating_sub(step.min(SIM_LENGTH));
    let time_scale = t_remaining.saturating_mul(1000) / SIM_LENGTH;

    // Asymmetric: only widen the vulnerable side (arb can exploit)
    let side_vulnerable = (pool_long_x && side == 0) || (!pool_long_x && side == 1);
    let raw_extra = if side_vulnerable {
        let imb_extra = (imb_permille / 100).saturating_mul(EXTRA_COEFF_BPS);
        imb_extra.saturating_mul(time_scale) / 1000
    } else {
        0
    };

    let extra = raw_extra.min(MAX_EXTRA_BPS);
    FEE_DENOMINATOR.saturating_sub(BASE_FEE_BPS).saturating_sub(extra)
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

    // Read engine step from storage[0..8] (mirrored by after_swap)
    let step = u64::from_le_bytes(
        decoded.storage[0..8].try_into().unwrap_or([0u8; 8])
    ) as u128;

    let k = reserve_x * reserve_y;
    let fee_num = fee_num_for(side, reserve_x, reserve_y, step);

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
