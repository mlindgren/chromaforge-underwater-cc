// SPDX-License-Identifier: GPL-3.0-only
//
// sRGB transfer function (IEC 61966-2-1), extended to all reals by odd symmetry and by continuing
// the curve above 1, so scene-referred highlights survive a round trip.
//
// Part of underwater-cc, a Rust port of underwater-image-color-correction by Nikolaj Bech Andersen
// (https://github.com/nikolajbech/underwater-image-color-correction). GPL-3.0-only; see LICENSE.

/// Linear light → sRGB-encoded.
#[inline]
pub fn encode(v: f32) -> f32 {
    let a = v.abs();
    let e = if a <= 0.003_130_8 {
        a * 12.92
    } else {
        1.055 * a.powf(1.0 / 2.4) - 0.055
    };
    e.copysign(v)
}

/// sRGB-encoded → linear light.
#[inline]
pub fn decode(v: f32) -> f32 {
    let a = v.abs();
    let l = if a <= 0.040_45 {
        a / 12.92
    } else {
        ((a + 0.055) / 1.055).powf(2.4)
    };
    l.copysign(v)
}

/// `x^p` for `x` in `[2^-9, 2^9)` by linear interpolation in a table with 256 segments per octave
/// (relative error below 1e-5 for the sRGB exponents), `powf` outside it.
struct PowTable {
    p: f32,
    vals: Vec<f32>,
}

const LO_BITS: u32 = 0x3B00_0000; // 2^-9
const HI: f32 = 512.0; // 2^9
const SHIFT: u32 = 15; // 23 mantissa bits, top 8 index the segment
const SEGMENTS: usize = 18 << 8;

impl PowTable {
    fn new(p: f32) -> Self {
        let vals = (0..=SEGMENTS)
            .map(|i| f32::from_bits(LO_BITS + ((i as u32) << SHIFT)).powf(p))
            .collect();
        Self { p, vals }
    }

    #[inline]
    fn pow(&self, x: f32) -> f32 {
        let bits = x.to_bits();
        if x.is_nan() || x >= HI || bits < LO_BITS {
            return x.powf(self.p);
        }
        let off = bits - LO_BITS;
        let i = (off >> SHIFT) as usize;
        let frac = (off & ((1 << SHIFT) - 1)) as f32 * (1.0 / (1u32 << SHIFT) as f32);
        match (self.vals.get(i), self.vals.get(i + 1)) {
            (Some(a), Some(b)) => a + (b - a) * frac,
            _ => x.powf(self.p),
        }
    }
}

/// [`encode`] and [`decode`] through tables: what the filter uses per sample (the interpreter the
/// host runs modules in makes `powf` the bulk of the cost otherwise).
pub struct Curves {
    enc: PowTable,
    dec: PowTable,
}

impl Curves {
    pub fn new() -> Self {
        Self {
            enc: PowTable::new(1.0 / 2.4),
            dec: PowTable::new(2.4),
        }
    }

    #[inline]
    pub fn encode(&self, v: f32) -> f32 {
        let a = v.abs();
        let e = if a <= 0.003_130_8 {
            a * 12.92
        } else {
            1.055 * self.enc.pow(a) - 0.055
        };
        e.copysign(v)
    }

    #[inline]
    pub fn decode(&self, v: f32) -> f32 {
        let a = v.abs();
        let l = if a <= 0.040_45 {
            a / 12.92
        } else {
            self.dec.pow((a + 0.055) / 1.055)
        };
        l.copysign(v)
    }
}

impl Default for Curves {
    fn default() -> Self {
        Self::new()
    }
}

/// Linear light → 8-bit sRGB code value (`round(encode(v) * 255)`, clamped; NaN → 0) without a
/// `powf` per sample: a binary search over the 255 decision levels in linear light.
pub struct Quantizer {
    /// `levels[i]`: the linear value where code `i` turns into code `i + 1`.
    levels: [f32; 255],
}

impl Quantizer {
    pub fn new() -> Self {
        Self {
            levels: core::array::from_fn(|i| decode((i as f32 + 0.5) / 255.0)),
        }
    }

    #[inline]
    pub fn srgb8(&self, v: f32) -> u8 {
        if v.is_nan() {
            return 0;
        }
        // the number of levels at or below v
        self.levels.partition_point(|l| *l <= v) as u8
    }
}

impl Default for Quantizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_including_highlights_and_negatives() {
        for i in -100..=400 {
            let v = i as f32 / 100.0;
            assert!(
                (decode(encode(v)) - v).abs() < 1e-5 * v.abs().max(1.0),
                "{v}"
            );
        }
        assert!((encode(1.0) - 1.0).abs() < 1e-6);
        assert!((encode(0.5) - 0.735_356_7).abs() < 1e-5);
    }

    #[test]
    fn the_tables_follow_the_curves() {
        let c = Curves::new();
        let mut v = 1e-6f32;
        while v < 2000.0 {
            for x in [v, -v] {
                let (e, d) = (encode(x), decode(x));
                assert!(
                    (c.encode(x) - e).abs() <= 1e-5 * e.abs().max(1e-3),
                    "encode {x}: {} vs {e}",
                    c.encode(x)
                );
                assert!(
                    (c.decode(x) - d).abs() <= 1e-5 * d.abs().max(1e-3),
                    "decode {x}: {} vs {d}",
                    c.decode(x)
                );
            }
            v *= 1.0007;
        }
        assert!(c.encode(f32::NAN).is_nan() && c.decode(f32::INFINITY) == f32::INFINITY);
    }

    #[test]
    fn the_quantizer_agrees_with_the_curve() {
        let q = Quantizer::new();
        for i in 0..=100_000 {
            let v = i as f32 / 80_000.0 - 0.1;
            let want = (encode(v) * 255.0).round().clamp(0.0, 255.0) as u8;
            let got = q.srgb8(v);
            assert!(got.abs_diff(want) <= 1, "{v}: {got} vs {want}");
        }
        for v in 0..=255u8 {
            assert_eq!(q.srgb8(decode(f32::from(v) / 255.0)), v);
        }
        assert_eq!(
            (q.srgb8(f32::NAN), q.srgb8(-1.0), q.srgb8(f32::INFINITY)),
            (0, 0, 255)
        );
    }

    #[test]
    fn eight_bit_values_survive_the_trip_through_linear() {
        for v in 0..=255u8 {
            let back = encode(decode(f32::from(v) / 255.0)) * 255.0;
            assert!((back - f32::from(v)).abs() < 1e-3, "{v} -> {back}");
        }
    }
}
