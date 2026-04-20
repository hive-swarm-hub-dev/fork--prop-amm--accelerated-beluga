use pinocchio::{account_info::AccountInfo, entrypoint, pubkey::Pubkey, ProgramResult};
use prop_amm_submission_sdk::{set_return_data_bytes, set_return_data_u64};

const NAME: &str = "Angeris Quad Invariant";
const MODEL_USED: &str = "claude-sonnet-4-6";

// Invariant: k = x*y + alpha*(100*x - y)^2 / 10000
// At equilibrium (100:1) the alpha term is 0, so k = x*y (identical to CPMM).
// Away from equilibrium, the alpha term creates a "stiffer" curve.
// NOTE: analysis shows this actually gives SLIGHTLY MORE output than CPMM
// for trades that worsen imbalance (opposite of the Angeris-penalize intuition),
// but still satisfies monotonicity and concavity, and may improve benchmark edge
// by tightening the effective spread near equilibrium.
//
// alpha <= 25 guarantees discriminant >= 0 for all reserve states and trade sizes.
const ALPHA: i128 = 10;
const FEE_DENOMINATOR: u128 = 10000;
const BASE_FEE_NUM: u128 = 9940; // 60 bps fee

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
        0 | 1 => {
            let output = compute_swap(instruction_data);
            set_return_data_u64(output);
        }
        2 => {}
        3 => set_return_data_bytes(NAME.as_bytes()),
        4 => set_return_data_bytes(get_model_used().as_bytes()),
        _ => {}
    }

    Ok(())
}

pub fn get_model_used() -> &'static str {
    MODEL_USED
}

/// Integer square root (floor) for non-negative i128.
fn isqrt_i128(n: i128) -> i128 {
    if n <= 0 {
        return 0;
    }
    let n_u = n as u128;
    let mut x = n_u;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n_u / x) / 2;
    }
    x as i128
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

    // Apply 60 bps fee
    let net_input = input_amount * BASE_FEE_NUM / FEE_DENOMINATOR;

    match side {
        0 => compute_side0(reserve_x, reserve_y, net_input),
        1 => compute_side1(reserve_x, reserve_y, net_input),
        _ => 0,
    }
}

/// Side 0: buy X by paying Y. Returns delta_x output.
///
/// Invariant: k = rx*ry + alpha*(100*rx - ry)^2/10000
/// After swap: (rx - delta_x, ry + net_y)
///
/// Quadratic in delta_x (scaled by 10000 to avoid fractions):
///   A_sc = 10000 * alpha
///   B_sc = -10000*(ry+ny) - 200*alpha*(b - ny)   where b = 100*rx - ry
///   C_sc = 10000*ny*rx - 2*alpha*b*ny + alpha*ny^2
///
/// Take smaller root: delta_x = (-B_sc - sqrt(disc)) / (2*A_sc)
///
/// Degenerates to CPMM when alpha=0: delta_x = ny*rx/(ry+ny).
fn compute_side0(rx: u128, ry: u128, net_y: u128) -> u64 {
    let rx_i = rx as i128;
    let ry_i = ry as i128;
    let ny_i = net_y as i128;
    let b = 100 * rx_i - ry_i; // pre-trade imbalance: 100*rx - ry

    // k = rx*ry + alpha*b^2/10000 (conceptually; used implicitly via C_sc)
    let a_sc: i128 = 10000 * ALPHA;
    let b_sc: i128 = -10000 * (ry_i + ny_i) - 200 * ALPHA * (b - ny_i);
    let c_sc: i128 = 10000 * ny_i * rx_i - 2 * ALPHA * b * ny_i + ALPHA * ny_i * ny_i;

    if a_sc == 0 {
        // alpha=0: pure CPMM
        let k = rx * ry;
        let new_ry = ry + net_y;
        let k_div = (k + new_ry - 1) / new_ry;
        return rx.saturating_sub(k_div) as u64;
    }

    let disc = b_sc * b_sc - 4 * a_sc * c_sc;
    if disc < 0 {
        return 0;
    }
    let sqrt_disc = isqrt_i128(disc);

    // Smaller root: delta_x = (-b_sc - sqrt_disc) / (2*a_sc)
    let numer = -b_sc - sqrt_disc;
    let denom = 2 * a_sc;
    if denom <= 0 || numer <= 0 {
        return 0;
    }
    let delta_x = numer / denom;
    if delta_x <= 0 {
        return 0;
    }
    (delta_x as u128).min(rx) as u64
}

/// Side 1: sell X, receive Y. Returns delta_y output.
///
/// Invariant: k = rx*ry + alpha*(100*rx - ry)^2/10000
/// After swap: (rx + net_x, ry - delta_y)
///
/// Let xp = rx + net_x (known). Solve for yp = ry - delta_y:
///   alpha*yp^2 + xp*(10000 - 200*alpha)*yp + (10000*alpha*xp^2 - 10000*k) = 0
///
/// Equivalently, quadratic in delta_y:
///   A_dy = alpha
///   B_dy = -(2*alpha*ry + xp*(10000 - 200*alpha))
///   C_dy = alpha*ry^2 + xp*(10000-200*alpha)*ry + 10000*alpha*xp^2 - 10000*k
///
/// Take smaller root: delta_y = (-B_dy - sqrt(disc)) / (2*A_dy)
///
/// Degenerates to CPMM when alpha=0: delta_y = net_x*ry/(rx+net_x).
/// For alpha <= 25, discriminant is guaranteed >= 0 for all valid reserve states.
fn compute_side1(rx: u128, ry: u128, net_x: u128) -> u64 {
    let rx_i = rx as i128;
    let ry_i = ry as i128;
    let nx_i = net_x as i128;
    let xp_i = rx_i + nx_i; // = rx + net_x

    let b0 = 100 * rx_i - ry_i; // pre-trade imbalance
    // k = rx*ry + alpha*b0^2/10000 (used as 10000*k below)
    // 10000*k = 10000*rx*ry + alpha*b0^2
    let k10000: i128 = 10000 * rx_i * ry_i + ALPHA * b0 * b0;

    let p: i128 = 10000 - 200 * ALPHA; // 10000 - 200*alpha

    if ALPHA == 0 {
        // Pure CPMM
        let k = rx * ry;
        let new_rx = rx + net_x;
        let k_div = (k + new_rx - 1) / new_rx;
        return ry.saturating_sub(k_div) as u64;
    }

    // A_dy = alpha
    let a_dy: i128 = ALPHA;
    // B_dy = -(2*alpha*ry + xp*(10000 - 200*alpha))
    let b_dy: i128 = -(2 * ALPHA * ry_i + xp_i * p);
    // C_dy = alpha*ry^2 + xp*p*ry + 10000*alpha*xp^2 - 10000*k
    //      = alpha*ry^2 + xp*p*ry + 10000*alpha*xp^2 - k10000
    let c_dy: i128 = ALPHA * ry_i * ry_i + xp_i * p * ry_i + 10000 * ALPHA * xp_i * xp_i - k10000;

    let disc = b_dy * b_dy - 4 * a_dy * c_dy;
    if disc < 0 {
        return 0;
    }
    let sqrt_disc = isqrt_i128(disc);

    // Smaller root: delta_y = (-B_dy - sqrt_disc) / (2*A_dy)
    let numer = -b_dy - sqrt_disc;
    let denom = 2 * a_dy;
    if denom <= 0 || numer <= 0 {
        return 0;
    }
    let delta_y = numer / denom;
    if delta_y <= 0 {
        return 0;
    }
    (delta_y as u128).min(ry) as u64
}
