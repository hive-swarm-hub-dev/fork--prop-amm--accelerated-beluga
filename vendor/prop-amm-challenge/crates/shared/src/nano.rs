pub const NANO_SCALE: u64 = 1_000_000_000;
pub const NANO_SCALE_F64: f64 = 1_000_000_000.0;

#[inline]
pub fn f64_to_nano(value: f64) -> u64 {
    if value.is_nan() || value <= 0.0 {
        return 0;
    }
    if value.is_infinite() {
        return u64::MAX;
    }
    let scaled = value * NANO_SCALE_F64;
    if scaled >= u64::MAX as f64 {
        u64::MAX
    } else {
        scaled as u64
    }
}

#[inline]
pub fn nano_to_f64(value: u64) -> f64 {
    value as f64 / NANO_SCALE_F64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip() {
        let original = 123.456789;
        let nano = f64_to_nano(original);
        let back = nano_to_f64(nano);
        assert!((original - back).abs() < 1e-9);
    }

    #[test]
    fn test_known_values() {
        assert_eq!(f64_to_nano(1.0), NANO_SCALE);
        assert_eq!(f64_to_nano(100.0), 100 * NANO_SCALE);
        assert_eq!(nano_to_f64(NANO_SCALE), 1.0);
    }

    #[test]
    fn test_invalid_values_clamp_to_zero() {
        assert_eq!(f64_to_nano(-1.0), 0);
        assert_eq!(f64_to_nano(f64::NAN), 0);
        assert_eq!(f64_to_nano(f64::INFINITY), u64::MAX);
    }
}
