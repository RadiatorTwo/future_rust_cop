//! Tone mapping (Glide): with a curve chosen, the 3D picture is drawn into a floating point
//! target (values above 1.0 kept: Texture Coloring's light up to 2x, additive effects) and
//! brought back into 0..1 by the curve before the HUD is drawn. The output stays an ordinary
//! 8-bit picture: any monitor shows it. Off (the default) is the path before: an 8-bit target,
//! no pass. While a curve is chosen, PlayStation Colors counts as off (the curve takes over
//! the colours); Texture Coloring and PlayStation Shading stay as set.
//!
//! REG_DWORD `Tonemap` in the game's key: 0 Off, 1 Soft (today's picture up to `KNEE`, a soft
//! shoulder above), 2 AgX, 3 ACES; `FCOP_TONEMAP=0..3` overrides it. Chosen in Advanced Graphics.

use super::advapi32;
use std::sync::atomic::{AtomicU32, Ordering};

/// A tone mapping curve.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Curve {
    Off = 0,
    Soft = 1,
    Agx = 2,
    Aces = 3,
}

impl Curve {
    /// The stored value's curve (unknown values: Off).
    pub fn from_value(v: u32) -> Curve {
        match v {
            1 => Curve::Soft,
            2 => Curve::Agx,
            3 => Curve::Aces,
            _ => Curve::Off,
        }
    }
}

/// The chosen curve's value; `u32::MAX` until read.
static CURVE: AtomicU32 = AtomicU32::new(u32::MAX);

pub fn curve() -> Curve {
    let v = CURVE.load(Ordering::Relaxed);
    if v != u32::MAX {
        return Curve::from_value(v);
    }
    let env = std::env::var("FCOP_TONEMAP").ok().and_then(|v| v.parse().ok());
    let stored = || advapi32::value("", "Tonemap").and_then(|(_, d)| Some(u32::from_le_bytes(d.get(..4)?.try_into().ok()?)));
    let c = Curve::from_value(env.or_else(stored).unwrap_or(0));
    CURVE.store(c as u32, Ordering::Relaxed);
    c
}

/// The Advanced Graphics dialog's OK: saved, and used from the next frame on.
pub fn set_curve(c: Curve) {
    CURVE.store(c as u32, Ordering::Relaxed);
    advapi32::set_value("", "Tonemap", 4, &(c as u32).to_le_bytes());
}

/// A curve is chosen: the picture is drawn in HDR.
pub fn hdr() -> bool {
    curve() != Curve::Off
}

/// The curves as Advanced Graphics lists them.
pub const CHOICES: [(Curve, &str); 4] = [(Curve::Off, "Off"), (Curve::Soft, "Soft"), (Curve::Agx, "AgX"), (Curve::Aces, "ACES")];

/// Soft: unchanged up to here.
pub const KNEE: f32 = 0.7;
/// Soft: where the shoulder is (effectively) white: Texture Coloring's brightest.
pub const WHITE: f32 = 2.0;

/// Soft on one value.
pub fn soft(x: f32) -> f32 {
    // an exponential shoulder: slope 1 at the knee, towards 1.0 (0.996 at WHITE), never above
    if x <= KNEE { x } else { KNEE + (1.0 - KNEE) * (1.0 - (-(x - KNEE) / (1.0 - KNEE)).exp()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_identity_below_knee() {
        for x in [0.0, 0.25, 0.5, KNEE] {
            assert_eq!(soft(x), x);
        }
    }

    #[test]
    fn soft_white_point() {
        let y = soft(WHITE);
        assert!(y > 0.99 && y <= 1.0, "{y}");
    }

    #[test]
    fn soft_bounded() {
        let mut last = 0.0;
        for k in 0..10_000 {
            let y = soft(k as f32 * 0.01);
            assert!(y >= last && y <= 1.0, "{k}: {y}");
            last = y;
        }
    }

    #[test]
    fn soft_smooth_at_knee() {
        let d = (soft(KNEE + 1e-3) - soft(KNEE)) / 1e-3;
        assert!((d - 1.0).abs() < 0.01, "{d}");
    }

    #[test]
    fn curve_from_value() {
        assert_eq!(Curve::from_value(2), Curve::Agx);
        assert_eq!(Curve::from_value(9), Curve::Off);
    }
}
