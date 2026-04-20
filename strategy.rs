use pinocchio::{account_info::AccountInfo, entrypoint, pubkey::Pubkey, ProgramResult};
use prop_amm_submission_sdk::{set_return_data_bytes, set_return_data_u64, set_storage};

const NAME: &str = "Avellaneda-Stoikov time-decaying spread";
const MODEL_USED: &str = "claude-sonnet-4-6";
const FEE_DENOMINATOR: u128 = 10000;
const STORAGE_SIZE: usize = 1024;

// A-S parameters
const BASE_FEE_BPS: u128 = 50;   // tighter base; time spread adds on top
const SIM_LENGTH: u128 = 10_000; // total steps T
const GAMMA_NUM: u128 = 1;       // risk aversion γ = 1
// σ² = 1e-6 per step; stored as 1 "per million"
// time_spread_bps = γ · σ² · (T-t) · 1e4/2 = γ · (T-t) / 200
// inventory_tilt_bps = (q_permille · γ · (T-t)) / 200_000
const MAX_EXTRA_BPS: u128 = 200;

fn read_step(storage: &[u8]) -> u64 {
    if storage.len() < 8 {
        return 0;
    }
    u64::from_le_bytes([
        storage[0], storage[1], storage[2], storage[3],
        storage[4], storage[5], storage[6], storage[7],
    ])
}

/// Compute A-S fee bps. side: 0=Y→X, 1=X→Y.
fn compute_fee_bps(side: u8, rx: u128, ry: u128, step: u128) -> u128 {
    let t_remaining = SIM_LENGTH.saturating_sub(step);

    // time spread: uniform component shrinks linearly
    let time_spread_bps = GAMMA_NUM.saturating_mul(t_remaining) / 200;

    // inventory imbalance: q measured as % deviation from y = rx*100 target
    let target_y = rx.saturating_mul(100);
    let (q_sign_positive, q_abs) = if target_y > ry {
        (true, target_y - ry)
    } else {
        (false, ry - target_y)
    };
    let q_permille = if ry == 0 { 0 } else { q_abs.saturating_mul(1000) / ry };
    // tilt_bps = q_permille · γ · (T-t) / 200_000
    let tilt_bps = q_permille
        .saturating_mul(GAMMA_NUM)
        .saturating_mul(t_remaining)
        / 200_000;

    // Apply tilt only to the side being exploited by inventory drift
    // q>0 (long X, ry too low): side 0 (user pays Y to get X) is the vulnerable side
    // q<0 (short X, ry too high): side 1 (user pays X to get Y) is vulnerable
    let side_matches = (q_sign_positive && side == 0) || (!q_sign_positive && side == 1);
    let inv_tilt = if side_matches { tilt_bps } else { 0 };

    let total_extra = (time_spread_bps + inv_tilt).min(MAX_EXTRA_BPS);
    (BASE_FEE_BPS + total_extra).min(500)
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
        0 | 1 => {
            let output = compute_swap(instruction_data);
            set_return_data_u64(output);
        }
        2 => {
            // after_swap layout: tag(1) side(1) input(8) output(8) rx(8) ry(8) step(8) storage(1024)
            if instruction_data.len() >= 42 + STORAGE_SIZE {
                let mut storage_buf = [0u8; STORAGE_SIZE];
                storage_buf.copy_from_slice(&instruction_data[42..42 + STORAGE_SIZE]);
                after_swap(instruction_data, &mut storage_buf);
                let _ = set_storage(&storage_buf);
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

/// Native mirror: increment step counter in storage bytes 0..8.
pub fn after_swap(_data: &[u8], storage: &mut [u8]) {
    if storage.len() < 8 {
        return;
    }
    let step = read_step(storage);
    let next = step.saturating_add(1);
    storage[0..8].copy_from_slice(&next.to_le_bytes());
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

    let step = read_step(&decoded._storage) as u128;
    let fee_bps = compute_fee_bps(side, reserve_x, reserve_y, step);
    let fee_num = FEE_DENOMINATOR.saturating_sub(fee_bps);
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
