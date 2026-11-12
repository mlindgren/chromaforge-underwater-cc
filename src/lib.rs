// SPDX-License-Identifier: GPL-3.0-only
//
// underwater-cc: underwater color correction as a Chromaforge pixel filter (PhotoCraft plug-in
// ABI v1, plus the v1.1 `pc_analyze` proposal), compiled to WebAssembly.
//
// A Rust port of underwater-image-color-correction by Nikolaj Bech Andersen,
// https://github.com/nikolajbech/underwater-image-color-correction (GPL-3.0). The algorithm is
// translated in `algo.rs`; this file adapts it to the host: measuring once per photo
// (`pc_analyze`) and applying the measured matrix to every band (`pc_filter`).
//
// Copyright (C) 2019 Nikolaj Bech Andersen (original JavaScript algorithm)
// Copyright (C) 2026 the underwater-cc contributors (Rust/WebAssembly port)
//
// This program is free software: you can redistribute it and/or modify it under the terms of the
// GNU General Public License as published by the Free Software Foundation, version 3.
// This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY;
// without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See
// the GNU General Public License (LICENSE) for more details.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable,
    clippy::indexing_slicing
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::indexing_slicing))]

pub mod algo;
pub mod color;

#[cfg(target_arch = "wasm32")]
mod abi;

use serde_json::{Map, Value, json};

use algo::{Analysis, Coefficients, DEFAULT_MIN_AVG_RED, MATRIX_LEN};

/// The ABI version this module implements.
pub const ABI_VERSION: i32 = 1;
/// `pc_filter`'s `format` for 32-bit float RGB without alpha: depth 32, colour mode 3 (RGB).
pub const FORMAT_RGB32: i32 = 32 | (3 << 16);
/// Version of the `_analysis` object this module writes and reads.
const ANALYSIS_VERSION: u32 = 1;

/// The manifest `pc_manifest` returns.
pub const MANIFEST: &str = concat!(
    r#"{"id":"underwater-color-correction","#,
    r#""name":"Underwater Color Correction","#,
    r#""version":""#,
    env!("CARGO_PKG_VERSION"),
    r#"","kind":"filter","#,
    r#""description":"Restores the reds and the contrast underwater photos lose: measures the photo (hue-shifts red in until its average red reaches the target, then stretches each channel between its histogram thresholds) and applies the resulting color matrix. Measure the photo first. A port of Nikolaj Bech Andersen's underwater-image-color-correction (GPL-3.0).","#,
    r#""author":"Nikolaj Bech Andersen (algorithm); underwater-cc contributors (Rust/WebAssembly port)","#,
    r#""license":"GPL-3.0-only","#,
    r#""homepage":"https://github.com/nikolajbech/underwater-image-color-correction","#,
    r#""repository":"https://github.com/mlindgren/chromaforge-underwater-cc","#,
    r#""overlap":0,"#,
    r#""stage":"display","#,
    r#""params":{"#,
    r#""intensity":{"type":"number","min":0,"max":150,"default":100,"label":"Intensity"},"#,
    r#""minAvgRed":{"type":"number","min":0,"max":128,"default":60,"label":"Red target (re-measure after changing)"}"#,
    r#"}}"#
);

/// Why a call failed (returned to the host as a nonzero code).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fail {
    /// The buffer's length, the size, channels or format don't describe an RGB f32 image.
    Shape = 1,
    /// The parameters aren't a JSON object.
    Params = 2,
    /// Out of memory, or a pointer the host gave is unusable.
    Memory = 3,
}

/// How the samples are encoded (the host's `encoding` key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// Linear light (Source stage; Display stage of HDR renders). Values may exceed 1.
    Linear,
    /// sRGB-curve encoded, 0..1 (Display stage).
    Srgb,
}

/// Checks the arguments `pc_filter` / `pc_analyze` get: the number of `f32` samples.
pub fn check_shape(len: i32, w: i32, h: i32, channels: i32, format: i32) -> Result<usize, Fail> {
    if channels != 3 || format != FORMAT_RGB32 || w < 0 || h < 0 || len < 0 {
        return Err(Fail::Shape);
    }
    let samples = (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(3))
        .ok_or(Fail::Shape)?;
    if samples.checked_mul(4) != Some(len as usize) {
        return Err(Fail::Shape);
    }
    Ok(samples)
}

/// The host's parameters (`{}` when there are none).
pub fn parse_params(bytes: &[u8]) -> Result<Map<String, Value>, Fail> {
    if bytes.is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(m)) => Ok(m),
        _ => Err(Fail::Params),
    }
}

fn number(p: &Map<String, Value>, key: &str, default: f64) -> f64 {
    p.get(key)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .unwrap_or(default)
}

fn encoding(p: &Map<String, Value>) -> Encoding {
    // The host always says; ABI v1 hands over linear light, so that's the fallback.
    match p.get("encoding").and_then(Value::as_str) {
        Some("srgb") => Encoding::Srgb,
        _ => Encoding::Linear,
    }
}

/// The measured matrix in `_analysis`, when it is one this module wrote.
pub fn analysis_matrix(p: &Map<String, Value>) -> Option<[f64; MATRIX_LEN]> {
    let list = p.get("_analysis")?.get("matrix")?.as_array()?;
    if list.len() != MATRIX_LEN {
        return None;
    }
    let mut m = [0.0; MATRIX_LEN];
    for (o, v) in m.iter_mut().zip(list) {
        *o = v.as_f64().filter(|f| f.is_finite())?;
    }
    Some(m)
}

/// `pc_analyze` on a buffer of interleaved RGB `f32` samples: the `_analysis` object, or `None`
/// when there is nothing to measure.
pub fn analyze(samples: &[f32], w: usize, h: usize, p: &Map<String, Value>) -> Option<Value> {
    // As the original sees a canvas: sRGB-encoded 8-bit values.
    let pixels: Vec<u8> = match encoding(p) {
        Encoding::Linear => {
            let q = color::Quantizer::new();
            samples.iter().map(|v| q.srgb8(*v)).collect()
        }
        Encoding::Srgb => samples
            .iter()
            .map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8)
            .collect(), // NaN → 0
    };
    let min_avg_red = number(p, "minAvgRed", DEFAULT_MIN_AVG_RED);
    let Analysis {
        matrix,
        hue_shift,
        average,
    } = algo::color_filter_matrix(&pixels, w, h, min_avg_red)?;
    Some(json!({
        "version": ANALYSIS_VERSION,
        "matrix": matrix.to_vec(),
        "hueShift": hue_shift,
        "averageColor": average.to_vec(),
        "minAvgRed": min_avg_red,
    }))
}

/// `pc_filter` on a buffer of interleaved RGB `f32` samples, in place. Without a measurement the
/// pixels pass through unchanged.
pub fn filter(samples: &mut [f32], p: &Map<String, Value>) {
    let Some(measured) = analysis_matrix(p) else {
        return;
    };
    let k = number(p, "intensity", 100.0) / 100.0;
    let c = Coefficients::from_matrix(&algo::blend_with_identity(&measured, k));
    let enc = encoding(p);
    let curves = color::Curves::new();
    for px in samples.chunks_exact_mut(3) {
        let [r, g, b] = [px.first(), px.get(1), px.get(2)].map(|v| v.copied().unwrap_or(0.0));
        let out = match enc {
            // Display stage: the original's own domain; clamp like its Uint8ClampedArray does.
            Encoding::Srgb => c.apply(r, g, b).map(|v| v.clamp(0.0, 1.0)),
            // Linear light: through sRGB and back. No negative light, but highlights above 1 stay.
            Encoding::Linear => c
                .apply(curves.encode(r), curves.encode(g), curves.encode(b))
                .map(|v| curves.decode(v.max(0.0))),
        };
        for (d, v) in px.iter_mut().zip(out) {
            *d = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_is_json_with_the_required_fields() {
        let m: Value = serde_json::from_str(MANIFEST).unwrap();
        assert_eq!(m["kind"], "filter");
        assert_eq!(m["license"], "GPL-3.0-only");
        assert_eq!(m["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(m["overlap"], 0);
        assert_eq!(m["params"]["intensity"]["max"], 150);
    }

    #[test]
    fn shapes_are_checked() {
        assert_eq!(check_shape(2 * 3 * 3 * 4, 2, 3, 3, FORMAT_RGB32), Ok(18));
        for (len, w, h, ch, f) in [
            (72, 2, 3, 3, 32),
            (72, 2, 3, 4, FORMAT_RGB32),
            (71, 2, 3, 3, FORMAT_RGB32),
            (0, -1, 0, 3, FORMAT_RGB32),
            (-4, 1, 1, 3, FORMAT_RGB32),
            (4, i32::MAX, i32::MAX, 3, FORMAT_RGB32),
        ] {
            assert_eq!(check_shape(len, w, h, ch, f), Err(Fail::Shape));
        }
    }

    #[test]
    fn params_must_be_an_object() {
        assert!(parse_params(b"").unwrap().is_empty());
        assert_eq!(parse_params(b"[1]"), Err(Fail::Params));
        assert_eq!(parse_params(b"{"), Err(Fail::Params));
    }

    #[test]
    fn no_or_malformed_measurement_passes_through() {
        let orig = vec![0.1, 0.5, 0.9, 2.0, -0.1, f32::NAN];
        for p in [
            r#"{}"#,
            r#"{"_analysis":null}"#,
            r#"{"_analysis":{"matrix":[1,2]}}"#,
            r#"{"_analysis":{"matrix":"x"}}"#,
        ] {
            let mut s = orig.clone();
            filter(&mut s, &parse_params(p.as_bytes()).unwrap());
            assert!(
                s.iter().zip(&orig).all(|(a, b)| a.to_bits() == b.to_bits()),
                "{p}"
            );
        }
    }

    #[test]
    fn linear_highlights_are_not_clipped_but_srgb_is() {
        let m = r#""_analysis":{"matrix":[1.2,0,0,0,0, 0,1,0,0,0, 0,0,1,0,0, 0,0,0,1,0]}"#;
        let mut lin = vec![3.0, 3.0, 3.0];
        filter(
            &mut lin,
            &parse_params(format!(r#"{{"encoding":"linear",{m}}}"#).as_bytes()).unwrap(),
        );
        assert!(lin[0] > 3.0 && (lin[1] - 3.0).abs() < 1e-4);
        let mut s = vec![0.9, 0.9, 0.9];
        filter(
            &mut s,
            &parse_params(format!(r#"{{"encoding":"srgb",{m}}}"#).as_bytes()).unwrap(),
        );
        assert_eq!(s[0], 1.0);
    }
}
