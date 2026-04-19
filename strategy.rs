use pinocchio::{account_info::AccountInfo, entrypoint, pubkey::Pubkey, ProgramResult};
use prop_amm_submission_sdk::{set_return_data_bytes, set_return_data_u64};

const NAME: &str = "My Strategy";
const MODEL_USED: &str = "GPT-5.3-Codex"; // Use "None" for fully human-written submissions.
const FEE_DENOMINATOR: u128 = 10000;
const BASE_FEE: u128 = 60;  // 60 bps floor
const EXTRA_PER_10PCT: u128 = 20;  // +20 bps per 10% imbalance
const MAX_EXTRA: u128 = 120;  // cap at +120 bps → max 180 bps
const STORAGE_SIZE: usize = 1024;

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
        // tag 2 = after_swap (no-op for this strategy)
        2 => {}
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

fn fee_num_for(reserve_x: u128, reserve_y: u128) -> u128 {
    let target_y = reserve_x.saturating_mul(100);
    let diff = if target_y > reserve_y { target_y - reserve_y } else { reserve_y - target_y };
    let imb_permille = if reserve_y == 0 { 0 } else { diff.saturating_mul(1000) / reserve_y };
    let extra = (imb_permille / 100).saturating_mul(EXTRA_PER_10PCT).min(MAX_EXTRA);
    FEE_DENOMINATOR.saturating_sub(BASE_FEE).saturating_sub(extra)
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
    let fee_num = fee_num_for(reserve_x, reserve_y);

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
