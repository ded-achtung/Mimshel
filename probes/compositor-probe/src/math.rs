pub const ICONS: usize = 8;
pub const BASE: f64 = 56.0;
pub const GAP: f64 = 14.0;

/// Compact, continuously differentiable falloff; inactive pointers return unity.
pub fn target_scale(distance: f64, active: bool) -> f64 {
    if !active {
        return 1.0;
    }
    let x = (distance.abs() / 150.0).min(1.0);
    1.0 + 0.8 * (1.0 - x * x).powi(2)
}

/// Exponential smoothing is independent of frame rate and cannot overshoot.
pub fn approach(current: f64, target: f64, dt: f64) -> f64 {
    current + (target - current) * (1.0 - (-dt.max(0.0) / 0.085).exp())
}

pub fn centers(width: f64) -> [f64; ICONS] {
    std::array::from_fn(|i| width / 2.0 + (i as f64 - 3.5) * (BASE + GAP))
}

/// Expand spacing alongside the icons to keep magnified neighbors from overlapping.
pub fn layout(width: f64, scales: &[f64; ICONS]) -> [(f64, f64); ICONS] {
    let total = scales.iter().sum::<f64>() * BASE + GAP * (ICONS - 1) as f64;
    let mut x = (width - total) / 2.0;
    std::array::from_fn(|i| {
        let side = BASE * scales[i];
        let item = (x, side);
        x += side + GAP;
        item
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn falloff_bounds_symmetry_and_edges() {
        assert_eq!(target_scale(0.0, true), 1.8);
        assert_eq!(target_scale(20.0, false), 1.0);
        for d in 0..500 {
            let s = target_scale(d as f64, true);
            assert!((1.0..=1.8).contains(&s));
            assert_eq!(s, target_scale(-(d as f64), true));
        }
        assert_eq!(target_scale(150.0, true), 1.0);
        assert!((target_scale(149.999, true) - 1.0).abs() < 1e-8);
    }
    #[test]
    fn animation_is_continuous_and_frame_rate_independent() {
        assert_eq!(approach(1.0, 1.8, 0.0), 1.0);
        let one = approach(1.0, 1.8, 0.1);
        let two = approach(approach(1.0, 1.8, 0.05), 1.8, 0.05);
        assert!((one - two).abs() < 1e-12);
        assert!(one > 1.0 && one < 1.8);
        assert!(approach(1.8, 1.0, 0.016) > 1.0);
    }
    #[test]
    fn layout_stays_centered_and_separated() {
        let l = layout(1280.0, &[1.8; ICONS]);
        assert!((l[0].0 - (1280.0 - l[7].0 - l[7].1)).abs() < 1e-10);
        for pair in l.windows(2) {
            assert!((pair[1].0 - pair[0].0 - pair[0].1 - GAP).abs() < 1e-10);
        }
    }
}
