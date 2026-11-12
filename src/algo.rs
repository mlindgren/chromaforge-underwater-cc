// SPDX-License-Identifier: GPL-3.0-only
//
// Underwater color correction: the color filter matrix.
//
// Translated from `index.js` of underwater-image-color-correction by Nikolaj Bech Andersen,
// https://github.com/nikolajbech/underwater-image-color-correction (GPL-3.0), version 1.0.3,
// commit b366f3252aebe0e76a1f824d431c94507cc5562b. The structure, magic numbers and floating-point
// operation order follow the original so the matrices match it; the differences are
//   * pixels are packed RGB (3 bytes) instead of canvas RGBA,
//   * the minimum red average is a parameter (the original's constant 60 is the default),
//   * the hue-shift search stops at the cap instead of relying on the original's literal 60,
//     which is the same thing for the default target and terminates for every other one,
//   * an empty image has no matrix instead of NaNs.
//
// This program is free software: you can redistribute it and/or modify it under the terms of the
// GNU General Public License as published by the Free Software Foundation, version 3.
// This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY;
// without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See
// the GNU General Public License (LICENSE) for more details.

use core::f64::consts::PI;

/// The original's `minAvgRed`: the hue shift moves red in until the average red reaches this.
pub const DEFAULT_MIN_AVG_RED: f64 = 60.0;
/// The original's `thresholdRatio`: a histogram bin counts as empty below `pixels / 2000`.
const THRESHOLD_RATIO: f64 = 2000.0;
/// The original's `maxHueShift`, in degrees.
pub const MAX_HUE_SHIFT: u32 = 120;
/// The original's `blueMagicValue`.
const BLUE_MAGIC_VALUE: f64 = 1.2;

/// Number of entries of the 4×5 color filter matrix (row-major, offsets in the 5th column).
pub const MATRIX_LEN: usize = 20;

/// What the analysis finds.
#[derive(Clone, Debug, PartialEq)]
pub struct Analysis {
    /// The 4×5 color filter matrix, exactly as the original returns it:
    /// `[rr, rg, rb, 0, ro,  0, gg, 0, 0, go,  0, 0, bb, 0, bo,  0, 0, 0, 1, 0]`.
    /// Gains apply to 0..255 values; offsets are fractions of full scale (the README adds
    /// `offset * 255` to 8-bit values, i.e. `offset` as is to 0..1 values).
    pub matrix: [f64; MATRIX_LEN],
    /// The hue shift the search ended at, in degrees.
    pub hue_shift: u32,
    /// The average color, in 0..255 units.
    pub average: [f64; 3],
}

#[derive(Clone, Copy)]
struct Rgb {
    r: f64,
    g: f64,
    b: f64,
}

/// The original's `hueShiftRed`: each channel scaled by a coefficient of the hue rotation into red.
fn hue_shift_red(r: f64, g: f64, b: f64, h: f64) -> Rgb {
    let u = (h * PI / 180.0).cos();
    let w = (h * PI / 180.0).sin();
    Rgb {
        r: (0.299 + 0.701 * u + 0.168 * w) * r,
        g: (0.587 - 0.587 * u + 0.330 * w) * g,
        b: (0.114 - 0.114 * u - 0.497 * w) * b,
    }
}

/// JavaScript's `Math.round` for the non-negative values it is used on here (halves round up).
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// The original's `normalizingInterval`: the widest gap between consecutive "empty" levels.
fn normalizing_interval(norm: &[u8]) -> (f64, f64) {
    let (mut high, mut low, mut max_dist) = (255i32, 0i32, 0i32);
    for pair in norm.windows(2) {
        if let [a, b] = pair {
            let dist = i32::from(*b) - i32::from(*a);
            if dist > max_dist {
                max_dist = dist;
                high = i32::from(*b);
                low = i32::from(*a);
            }
        }
    }
    (f64::from(low), f64::from(high))
}

/// The original's `getColorFilterMatrix` on packed 8-bit RGB `pixels` (`width × height × 3`
/// bytes). `None` when the image is empty or the buffer doesn't match the size.
pub fn color_filter_matrix(
    pixels: &[u8],
    width: usize,
    height: usize,
    min_avg_red: f64,
) -> Option<Analysis> {
    let num_of_pixels = width.checked_mul(height)?;
    if num_of_pixels == 0 || pixels.len() != num_of_pixels.checked_mul(3)? {
        return None;
    }
    let n = num_of_pixels as f64;
    let threshold_level = n / THRESHOLD_RATIO;

    // calculateAverageColor (sums of integers: exact in f64, so the order doesn't matter)
    let mut sum = [0f64; 3];
    for px in pixels.chunks_exact(3) {
        for (s, v) in sum.iter_mut().zip(px) {
            *s += f64::from(*v);
        }
    }
    let avg = Rgb {
        r: sum[0] / n,
        g: sum[1] / n,
        b: sum[2] / n,
    };

    // Calculate the shift amount (the original's loop, quirks included: the shift it returns is
    // one past the one that reached the target).
    let mut new_avg_red = avg.r;
    let mut hue_shift: u32 = 0;
    while new_avg_red < min_avg_red {
        let s = hue_shift_red(avg.r, avg.g, avg.b, f64::from(hue_shift));
        new_avg_red = s.r + s.g + s.b;
        hue_shift += 1;
        if hue_shift > MAX_HUE_SHIFT {
            break; // the original sets newAvgRed = 60 (its target) here
        }
    }
    let h = f64::from(hue_shift);

    // Histogram with the new red values. hueShiftRed's coefficients depend on h only.
    let unit = hue_shift_red(1.0, 1.0, 1.0, h);
    let mut hist = [[0u32; 256]; 3];
    for px in pixels.chunks_exact(3) {
        let [r, g, b] = [px.first(), px.get(1), px.get(2)].map(|v| v.copied().unwrap_or(0));
        let red = unit.r * f64::from(r) + unit.g * f64::from(g) + unit.b * f64::from(b);
        let red = js_round(red.clamp(0.0, 255.0)) as usize; // 0..=255; `as` saturates
        for (c, bin) in [(0, red), (1, usize::from(g)), (2, usize::from(b))] {
            if let Some(count) = hist.get_mut(c).and_then(|hc| hc.get_mut(bin)) {
                *count += 1;
            }
        }
    }

    // Levels under the threshold, framed by 0 and 255.
    let mut adjust = [(0.0, 255.0); 3];
    for (c, hc) in hist.iter().enumerate() {
        let mut norm: Vec<u8> = Vec::with_capacity(258);
        norm.push(0);
        for (i, count) in (0u8..=255).zip(hc.iter()) {
            if f64::from(*count) - threshold_level < 2.0 {
                norm.push(i);
            }
        }
        norm.push(255);
        if let Some(a) = adjust.get_mut(c) {
            *a = normalizing_interval(&norm);
        }
    }
    let [(r_low, r_high), (g_low, g_high), (b_low, b_high)] = adjust;

    let red_gain = 256.0 / (r_high - r_low);
    let green_gain = 256.0 / (g_high - g_low);
    let blue_gain = 256.0 / (b_high - b_low);

    let red_offset = (-r_low / 256.0) * red_gain;
    let green_offset = (-g_low / 256.0) * green_gain;
    let blue_offset = (-b_low / 256.0) * blue_gain;

    let adjst_red = unit.r * red_gain;
    let adjst_red_green = unit.g * red_gain;
    let adjst_red_blue = unit.b * red_gain * BLUE_MAGIC_VALUE;

    #[rustfmt::skip]
    let matrix = [
        adjst_red, adjst_red_green, adjst_red_blue, 0.0, red_offset,
        0.0, green_gain, 0.0, 0.0, green_offset,
        0.0, 0.0, blue_gain, 0.0, blue_offset,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ];
    Some(Analysis {
        matrix,
        hue_shift,
        average: [avg.r, avg.g, avg.b],
    })
}

/// The matrix the filter applies at `intensity` (0 = identity, 1 = the measured matrix, more
/// extrapolates): `I + k (M − I)`.
pub fn blend_with_identity(m: &[f64; MATRIX_LEN], k: f64) -> [f64; MATRIX_LEN] {
    let mut out = [0.0; MATRIX_LEN];
    for (i, (o, v)) in out.iter_mut().zip(m).enumerate() {
        let id = if i % 6 == 0 { 1.0 } else { 0.0 }; // diagonal of a 4×5 row-major matrix: 0, 6, 12, 18
        *o = id + k * (v - id);
    }
    out
}

/// The coefficients of the original README's way of applying a matrix to normalised (0..1)
/// values: `r' = rr·r + rg·g + rb·b + ro`, `g' = gg·g + go`, `b' = bb·b + bo`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coefficients {
    pub rr: f32,
    pub rg: f32,
    pub rb: f32,
    pub ro: f32,
    pub gg: f32,
    pub go: f32,
    pub bb: f32,
    pub bo: f32,
}

impl Coefficients {
    pub fn from_matrix(m: &[f64; MATRIX_LEN]) -> Self {
        let at = |i: usize| m.get(i).copied().unwrap_or(0.0) as f32;
        Self {
            rr: at(0),
            rg: at(1),
            rb: at(2),
            ro: at(4),
            gg: at(6),
            go: at(9),
            bb: at(12),
            bo: at(14),
        }
    }

    /// The README's formula on one pixel of 0..1 values (no clamping).
    #[inline]
    pub fn apply(&self, r: f32, g: f32, b: f32) -> [f32; 3] {
        [
            r * self.rr + g * self.rg + b * self.rb + self.ro,
            g * self.gg + self.go,
            b * self.bb + self.bo,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_at_zero_and_measured_at_one() {
        let m: [f64; MATRIX_LEN] = core::array::from_fn(|i| i as f64 * 0.1 - 0.5);
        let z = blend_with_identity(&m, 0.0);
        for (i, v) in z.iter().enumerate() {
            assert_eq!(*v, if i % 6 == 0 { 1.0 } else { 0.0 });
        }
        assert_eq!(blend_with_identity(&m, 1.0), m);
    }

    #[test]
    fn empty_or_mismatched_images_have_no_matrix() {
        assert!(color_filter_matrix(&[], 0, 0, 60.0).is_none());
        assert!(color_filter_matrix(&[1, 2, 3], 2, 1, 60.0).is_none());
        assert!(color_filter_matrix(&[1, 2, 3], usize::MAX, 2, 60.0).is_none());
    }

    #[test]
    fn the_search_ends_for_unreachable_targets() {
        // Black: red never rises; the original would spin forever with a target above 60.
        let a = color_filter_matrix(&[0; 300], 10, 10, 200.0).unwrap();
        assert_eq!(a.hue_shift, MAX_HUE_SHIFT + 1);
        assert!(a.matrix.iter().all(|v| v.is_finite()));
        let nan = color_filter_matrix(&[0; 300], 10, 10, f64::NAN).unwrap();
        assert_eq!(nan.hue_shift, 0);
    }

    #[test]
    fn a_red_enough_image_is_not_shifted() {
        let px: Vec<u8> = (0..64 * 64)
            .flat_map(|i| [(i % 256) as u8, 128, 100])
            .collect();
        let a = color_filter_matrix(&px, 64, 64, 60.0).unwrap();
        assert_eq!(a.hue_shift, 0);
        // with no shift, the red row takes red only
        assert!((a.matrix[1]).abs() < 1e-12 && (a.matrix[2]).abs() < 1e-12);
    }
}
