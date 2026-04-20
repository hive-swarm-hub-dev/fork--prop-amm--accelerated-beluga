use pinocchio::{account_info::AccountInfo, entrypoint, pubkey::Pubkey, ProgramResult};
use prop_amm_submission_sdk::{set_return_data_bytes, set_return_data_u64, set_storage};

const NAME: &str = "DODO PMM EMA Oracle";
const MODEL_USED: &str = "claude-sonnet-4-6";
const FEE_NUMERATOR: u128 = 9930; // 70 bps base fee
const FEE_DENOMINATOR: u128 = 10000;
const K_SLIP_NUM: u128 = 500; // 50% blend toward oracle-ideal
const K_SLIP_DEN: u128 = 1000;
const EMA_ALPHA_INV: u128 = 16; // α = 1/16
const STORAGE_SIZE: usize = 1024;

// Storage layout:
// bytes 0..8: target_ratio_q32 (Q32.32 fixed-point: ry/rx)
// bytes 8..16: update_count (u64)

fn isqrt_u128(n: u128) -> u128 {
    if n == 0 {
        return 0;
    }
    let bits = 128u32.saturating_sub(n.leading_zeros());
    let mut x = 1u128 << ((bits + 1) / 2);
    loop {
        let next = (x + n / x) / 2;
        if next >= x {
            return x;
        }
        x = next;
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
        0 | 1 => {
            let output = compute_swap(instruction_data);
            set_return_data_u64(output);
        }
        2 => {
            // BPF path: parse storage from instruction_data offset 42, then set_storage
            bpf_after_swap(instruction_data);
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

// BPF path: called via process_instruction tag=2
// afterSwap layout:
// [0]     tag (2)
// [1]     side
// [2..10] input_amount
// [10..18] output_amount
// [18..26] reserve_x
// [26..34] reserve_y
// [34..42] step
// [42..1066] storage (1024 bytes)
fn bpf_after_swap(data: &[u8]) {
    if data.len() < 42 + 16 {
        return;
    }
    let reserve_x = u64::from_le_bytes(data[18..26].try_into().unwrap_or([0u8; 8]));
    let reserve_y = u64::from_le_bytes(data[26..34].try_into().unwrap_or([0u8; 8]));
    if reserve_x == 0 {
        return;
    }
    let old_ema_q32 = u64::from_le_bytes(data[42..50].try_into().unwrap_or([0u8; 8]));
    let update_count = u64::from_le_bytes(data[50..58].try_into().unwrap_or([0u8; 8]));

    let instant_q32 = ((reserve_y as u128) << 32) / (reserve_x as u128);
    let instant_q32 = instant_q32.min(u64::MAX as u128) as u64;

    let new_ema_q32 = if update_count == 0 {
        instant_q32
    } else {
        let old = old_ema_q32 as u128;
        let instant = instant_q32 as u128;
        ((old * (EMA_ALPHA_INV - 1) + instant) / EMA_ALPHA_INV).min(u64::MAX as u128) as u64
    };

    let new_count = update_count.saturating_add(1);
    let mut buf = [0u8; STORAGE_SIZE];
    buf[0..8].copy_from_slice(&new_ema_q32.to_le_bytes());
    buf[8..16].copy_from_slice(&new_count.to_le_bytes());
    let _ = set_storage(&buf);
}

// Native path: called by the shim with storage passed as mutable slice
pub fn after_swap(data: &[u8], storage: &mut [u8]) {
    // afterSwap layout (same as BPF but storage is passed separately):
    // data[0]     tag (2)
    // data[1]     side
    // data[2..10] input_amount
    // data[10..18] output_amount
    // data[18..26] reserve_x
    // data[26..34] reserve_y
    // data[34..42] step
    if data.len() < 34 {
        return;
    }
    let reserve_x = u64::from_le_bytes(data[18..26].try_into().unwrap_or([0u8; 8]));
    let reserve_y = u64::from_le_bytes(data[26..34].try_into().unwrap_or([0u8; 8]));
    if reserve_x == 0 {
        return;
    }

    // Read current state from storage
    let old_ema_q32 = if storage.len() >= 8 {
        u64::from_le_bytes(storage[0..8].try_into().unwrap_or([0u8; 8]))
    } else {
        0
    };
    let update_count = if storage.len() >= 16 {
        u64::from_le_bytes(storage[8..16].try_into().unwrap_or([0u8; 8]))
    } else {
        0
    };

    let instant_q32 = ((reserve_y as u128) << 32) / (reserve_x as u128);
    let instant_q32 = instant_q32.min(u64::MAX as u128) as u64;

    let new_ema_q32 = if update_count == 0 {
        instant_q32
    } else {
        let old = old_ema_q32 as u128;
        let instant = instant_q32 as u128;
        ((old * (EMA_ALPHA_INV - 1) + instant) / EMA_ALPHA_INV).min(u64::MAX as u128) as u64
    };

    let new_count = update_count.saturating_add(1);

    if storage.len() >= 16 {
        storage[0..8].copy_from_slice(&new_ema_q32.to_le_bytes());
        storage[8..16].copy_from_slice(&new_count.to_le_bytes());
    }
}

pub fn compute_swap(data: &[u8]) -> u64 {
    let decoded: ComputeSwapInstruction = match wincode::deserialize(data) {
        Ok(d) => d,
        Err(_) => return 0,
    };

    let rx = decoded.reserve_x as u128;
    let ry = decoded.reserve_y as u128;
    if rx == 0 || ry == 0 {
        return 0;
    }

    let k_inv = match rx.checked_mul(ry) {
        Some(k) => k,
        None => return 0,
    };

    // Read oracle target ratio Q32.32 (ry/rx), fallback to 100 (100 Y per X)
    let target_q32: u128 = {
        let raw = u64::from_le_bytes(decoded.storage[0..8].try_into().unwrap_or([0u8; 8]));
        if raw == 0 {
            100u128 << 32
        } else {
            raw as u128
        }
    };

    // Ideal reserves on CPMM such that ry'/rx' = target (as Q32.32)
    // k_inv = rx' * ry', ry'/rx' = target_q32 / 2^32
    // => rx'^2 * target_q32 = k_inv * 2^32
    // => rx' = sqrt(k_inv * 2^32 / target_q32)
    let ideal_rx: u128 = {
        // To avoid overflow: k_inv can be up to (2^64-1)^2 ≈ 2^128.
        // (2^64-1)^2 * 2^32 overflows u128. So split carefully.
        // Use: sqrt(k_inv * 2^32 / target_q32) = sqrt(k_inv / target_q32) * 2^16
        // which avoids the intermediate overflow.
        // More precisely: let a = k_inv / target_q32 (integer division),
        // let b = k_inv % target_q32.
        // sqrt(k_inv * 2^32 / target_q32) = sqrt(a * 2^32 + b * 2^32 / target_q32)
        // ≈ sqrt(a * 2^32) = isqrt(a) * 2^16 (for large values, fractional part negligible)
        // But for small k_inv / target_q32, we need precision.
        // Alternative: if k_inv <= u128::MAX >> 32, shift directly.
        let scaled = if k_inv <= (u128::MAX >> 32) {
            (k_inv << 32) / target_q32
        } else {
            // Divide first: lose at most 1 bit of precision in sqrt
            (k_inv / target_q32) << 32
        };
        isqrt_u128(scaled)
    };

    let ideal_ry: u128 = if ideal_rx == 0 {
        ry
    } else {
        k_inv / ideal_rx
    };

    // Blend: virt = (1 - k_slip) * real + k_slip * ideal
    let virt_rx = (rx * (K_SLIP_DEN - K_SLIP_NUM) + ideal_rx * K_SLIP_NUM) / K_SLIP_DEN;
    let virt_ry = (ry * (K_SLIP_DEN - K_SLIP_NUM) + ideal_ry * K_SLIP_NUM) / K_SLIP_DEN;

    // Safety: if virt > 2*real, cap it (shouldn't happen in normal operation)
    let virt_rx = virt_rx.min(rx * 2);
    let virt_ry = virt_ry.min(ry * 2);

    let virt_k = match virt_rx.checked_mul(virt_ry) {
        Some(k) => k,
        None => return 0,
    };

    let input = decoded.input_amount as u128;
    let net = input * FEE_NUMERATOR / FEE_DENOMINATOR;

    match decoded.side {
        0 => {
            // Sell Y (input = Y), get X
            let new_virt_ry = virt_ry + net;
            let new_virt_rx = (virt_k + new_virt_ry - 1) / new_virt_ry;
            let delta = virt_rx.saturating_sub(new_virt_rx);
            delta.min(rx) as u64
        }
        1 => {
            // Sell X (input = X), get Y
            let new_virt_rx = virt_rx + net;
            let new_virt_ry = (virt_k + new_virt_rx - 1) / new_virt_rx;
            let delta = virt_ry.saturating_sub(new_virt_ry);
            delta.min(ry) as u64
        }
        _ => 0,
    }
}
