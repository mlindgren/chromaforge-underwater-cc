// SPDX-License-Identifier: GPL-3.0-only
//
// The filter as Chromaforge runs it: the .wasm module loaded by the real host (`lcc-filters`),
// measured with `analyze_rgb` and applied with `apply_rgb`, checked against fixtures made by the
// ORIGINAL JavaScript (tools/reference/make-reference.js on index.js of
// https://github.com/nikolajbech/underwater-image-color-correction by Nikolaj Bech Andersen).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;

use lcc_filters::{Filter, FrameInfo, Limits, ParamSpec, Stage};
use serde_json::{Value, json};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The module, built for wasm32 (release) once per test run, as `cargo xtask build` builds it.
fn wasm() -> &'static [u8] {
    static WASM: OnceLock<Vec<u8>> = OnceLock::new();
    WASM.get_or_init(|| {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = std::process::Command::new(cargo)
            .current_dir(root())
            .args([
                "build",
                "--release",
                "--lib",
                "-p",
                "underwater-cc",
                "--target",
                "wasm32-unknown-unknown",
            ])
            .status()
            .expect("run cargo");
        assert!(
            status.success(),
            "building the wasm module failed (rustup target add wasm32-unknown-unknown?)"
        );
        std::fs::read(root().join("target/wasm32-unknown-unknown/release/underwater_cc.wasm"))
            .expect("read the module")
    })
}

fn load(limits: Limits) -> Filter {
    Filter::load(wasm(), limits).expect("the host loads the module")
}

/// An 8-bit RGB PNG fixture: (pixels, width, height).
fn png(path: &Path) -> (Vec<u8>, u32, u32) {
    let dec = png::Decoder::new(
        std::fs::File::open(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    );
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    assert_eq!(
        (info.color_type, info.bit_depth),
        (png::ColorType::Rgb, png::BitDepth::Eight),
        "{}",
        path.display()
    );
    buf.truncate(info.buffer_size());
    (buf, info.width, info.height)
}

struct Reference {
    name: String,
    pixels: Vec<u8>,
    filtered: Vec<u8>,
    w: u32,
    h: u32,
    matrix: Vec<f64>,
}

fn fixtures() -> PathBuf {
    root().join("tests/fixtures")
}

fn references() -> Vec<Reference> {
    let r: Value =
        serde_json::from_slice(&std::fs::read(fixtures().join("reference.json")).unwrap()).unwrap();
    r["images"]
        .as_array()
        .unwrap()
        .iter()
        .map(|im| {
            let name = im["name"].as_str().unwrap().to_string();
            let (pixels, w, h) = png(&fixtures().join(format!("{name}.png")));
            let (filtered, fw, fh) = png(&fixtures().join(format!("{name}.js-filtered.png")));
            assert_eq!(
                (w, h, fw, fh),
                (
                    im["width"].as_u64().unwrap() as u32,
                    im["height"].as_u64().unwrap() as u32,
                    w,
                    h
                )
            );
            let matrix = im["matrix"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect();
            Reference {
                name,
                pixels,
                filtered,
                w,
                h,
                matrix,
            }
        })
        .collect()
}

/// sRGB-encoded → linear, as the host linearises an 8-bit JPEG.
fn to_linear(v: f32) -> f32 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn linear(px: &[u8]) -> Vec<f32> {
    px.iter()
        .map(|v| to_linear(f32::from(*v) / 255.0))
        .collect()
}
fn srgb(px: &[u8]) -> Vec<f32> {
    px.iter().map(|v| f32::from(*v) / 255.0).collect()
}

fn assert_matrix(name: &str, got: &Value, want: &[f64]) {
    let got: Vec<f64> = got["matrix"]
        .as_array()
        .unwrap_or_else(|| panic!("{name}: no matrix in {got}"))
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert_eq!(got.len(), 20);
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert!(
            (g - w).abs() < 1e-9,
            "{name}: matrix[{i}] = {g}, the JS gives {w}\n got {got:?}\nwant {want:?}"
        );
    }
}

/// Small bands, so one image takes dozens of fresh instances.
fn small_bands() -> Limits {
    Limits {
        band_bytes: 48 * 1024,
        ..Limits::default()
    }
}

#[test]
fn the_module_has_no_imports_and_every_export() {
    let mut exports = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(wasm()) {
        match payload.unwrap() {
            wasmparser::Payload::ImportSection(s) => {
                assert_eq!(s.count(), 0, "the module imports something")
            }
            wasmparser::Payload::ExportSection(s) => {
                exports.extend(s.into_iter().map(|e| e.unwrap().name.to_string()))
            }
            _ => {}
        }
    }
    for want in [
        "memory",
        "pc_abi_version",
        "pc_manifest",
        "pc_alloc",
        "pc_filter",
        "pc_analyze",
    ] {
        assert!(
            exports.iter().any(|e| e == want),
            "missing export {want}: {exports:?}"
        );
    }
    assert!(lcc_filters::is_filter_module(wasm()));
}

#[test]
fn the_host_reads_the_manifest() {
    let f = load(Limits::default());
    let m = f.manifest();
    assert_eq!(m.id, "underwater-color-correction");
    assert_eq!(m.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(m.overlap, 0);
    assert_eq!(m.stage, Some(Stage::Display));
    assert!(m.dropped.is_empty(), "the host dropped {:?}", m.dropped);
    assert_eq!(m.about.license.as_deref(), Some("GPL-3.0-only"));
    assert_eq!(
        m.about.repository.as_deref(),
        Some(env!("CARGO_PKG_REPOSITORY"))
    );
    assert!(f.has_analyze() && !f.has_merge());
    let names: Vec<&str> = m.params.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, ["intensity", "minAvgRed"]);
    assert_eq!(
        m.params[0].1,
        ParamSpec::Number {
            min: 0.0,
            max: 150.0,
            default: 100.0
        }
    );
    assert_eq!(
        m.params[1].1,
        ParamSpec::Number {
            min: 0.0,
            max: 128.0,
            default: 60.0
        }
    );
    let resolved = m.resolve_params(&json!({"intensity": 400})).unwrap();
    assert_eq!(resolved["intensity"], 150.0);
}

#[test]
fn the_algorithm_matches_the_javascript_exactly() {
    for r in references() {
        let a =
            underwater_cc::algo::color_filter_matrix(&r.pixels, r.w as usize, r.h as usize, 60.0)
                .unwrap();
        assert_matrix(&r.name, &json!({"matrix": a.matrix.to_vec()}), &r.matrix);
    }
}

#[test]
fn pc_analyze_in_the_host_matches_the_javascript() {
    let never = AtomicBool::new(false);
    let f = load(Limits::default());
    for r in references() {
        // As Chromaforge measures: linear light, encoding "linear".
        let a = f
            .analyze_rgb(
                &linear(&r.pixels),
                r.w,
                r.h,
                &json!({"encoding": "linear"}),
                &never,
            )
            .unwrap()
            .unwrap();
        assert_matrix(&r.name, &a, &r.matrix);
        assert_eq!(a["minAvgRed"], 60.0);
        // An sRGB-encoded buffer measures the same.
        let s = f
            .analyze_rgb(
                &srgb(&r.pixels),
                r.w,
                r.h,
                &json!({"encoding": "srgb"}),
                &never,
            )
            .unwrap()
            .unwrap();
        assert_matrix(&r.name, &s, &r.matrix);
    }
}

#[test]
fn the_red_target_changes_the_measurement() {
    let never = AtomicBool::new(false);
    let f = load(Limits::default());
    let r = references()
        .into_iter()
        .find(|r| r.name == "example3")
        .unwrap();
    let px = linear(&r.pixels);
    let a60 = f
        .analyze_rgb(&px, r.w, r.h, &json!({"encoding": "linear"}), &never)
        .unwrap()
        .unwrap();
    let a90 = f
        .analyze_rgb(
            &px,
            r.w,
            r.h,
            &json!({"encoding": "linear", "minAvgRed": 90}),
            &never,
        )
        .unwrap()
        .unwrap();
    assert!(
        a90["hueShift"].as_u64() > a60["hueShift"].as_u64(),
        "{a60} vs {a90}"
    );
}

/// Largest difference, in 8-bit levels, between `got` (0..1 sRGB-encoded floats) and `want`.
fn max_diff(got: &[f32], want: &[u8]) -> f32 {
    got.iter()
        .zip(want)
        .map(|(g, w)| (g.clamp(0.0, 1.0) * 255.0 - f32::from(*w)).abs())
        .fold(0.0, f32::max)
}

fn run(f: &Filter, img: &mut [f32], r: &Reference, params: Value) {
    let never = AtomicBool::new(false);
    f.apply_rgb(img, r.w, r.h, FrameInfo::whole(r.w, r.h), &params, &never)
        .unwrap();
}

#[test]
fn intensity_0_is_identity_in_both_encodings() {
    let never = AtomicBool::new(false);
    let f = load(small_bands());
    for r in references() {
        let analysis = f
            .analyze_rgb(
                &linear(&r.pixels),
                r.w,
                r.h,
                &json!({"encoding": "linear"}),
                &never,
            )
            .unwrap()
            .unwrap();
        let orig = srgb(&r.pixels);
        let mut img = orig.clone();
        run(
            &f,
            &mut img,
            &r,
            json!({"encoding": "srgb", "intensity": 0, "_analysis": analysis}),
        );
        assert_eq!(img, orig, "{}: srgb", r.name);
        let orig = linear(&r.pixels);
        let mut img = orig.clone();
        run(
            &f,
            &mut img,
            &r,
            json!({"encoding": "linear", "intensity": 0, "_analysis": analysis}),
        );
        let worst = img
            .iter()
            .zip(&orig)
            .map(|(a, b)| (a - b).abs() / b.max(1e-3))
            .fold(0.0, f32::max);
        assert!(
            worst < 1e-4,
            "{}: linear identity off by {worst} (relative)",
            r.name
        );
    }
}

#[test]
fn intensity_100_matches_the_javascript_output_in_both_encodings() {
    let never = AtomicBool::new(false);
    for limits in [Limits::default(), small_bands()] {
        let f = load(limits);
        for r in references() {
            let analysis = f
                .analyze_rgb(
                    &linear(&r.pixels),
                    r.w,
                    r.h,
                    &json!({"encoding": "linear"}),
                    &never,
                )
                .unwrap()
                .unwrap();

            // Display stage: sRGB-encoded in, out (the original's own domain).
            let mut img = srgb(&r.pixels);
            run(
                &f,
                &mut img,
                &r,
                json!({"encoding": "srgb", "intensity": 100, "_analysis": analysis}),
            );
            assert!(
                img.iter().all(|v| (0.0..=1.0).contains(v)),
                "{}: srgb output must be clamped",
                r.name
            );
            let d = max_diff(&img, &r.filtered);
            assert!(
                d <= 0.51,
                "{}: srgb output differs from the JS by {d} levels",
                r.name
            );

            // Source stage: linear in, linear out.
            let mut img = linear(&r.pixels);
            run(
                &f,
                &mut img,
                &r,
                json!({"encoding": "linear", "_analysis": analysis}),
            ); // intensity: default 100
            let enc: Vec<f32> = img.iter().map(|v| to_srgb(*v)).collect();
            let d = max_diff(&enc, &r.filtered);
            assert!(
                d <= 0.52,
                "{}: linear output differs from the JS by {d} levels",
                r.name
            );
        }
    }
}

#[test]
fn bands_do_not_change_the_result() {
    let never = AtomicBool::new(false);
    let r = references()
        .into_iter()
        .find(|r| r.name == "example3")
        .unwrap();
    let whole = load(Limits {
        band_bytes: 64 << 20,
        ..Limits::default()
    });
    let analysis = whole
        .analyze_rgb(
            &linear(&r.pixels),
            r.w,
            r.h,
            &json!({"encoding": "linear"}),
            &never,
        )
        .unwrap()
        .unwrap();
    let params = json!({"encoding": "linear", "intensity": 70, "_analysis": analysis});
    let mut a = linear(&r.pixels);
    run(&whole, &mut a, &r, params.clone());
    let mut b = linear(&r.pixels);
    run(
        &load(Limits {
            band_bytes: 12 * 1024,
            ..Limits::default()
        }),
        &mut b,
        &r,
        params,
    );
    assert_eq!(a, b);
}

#[test]
fn linear_highlights_above_one_survive() {
    let never = AtomicBool::new(false);
    let f = load(small_bands());
    let r = references().into_iter().next().unwrap();
    let analysis = f
        .analyze_rgb(
            &linear(&r.pixels),
            r.w,
            r.h,
            &json!({"encoding": "linear"}),
            &never,
        )
        .unwrap()
        .unwrap();
    // a scene-referred version of the photo: everything 4× brighter
    let mut img: Vec<f32> = linear(&r.pixels).iter().map(|v| v * 4.0).collect();
    run(
        &f,
        &mut img,
        &r,
        json!({"encoding": "linear", "_analysis": analysis}),
    );
    assert!(img.iter().any(|v| *v > 1.5), "highlights were clipped");
    assert!(img.iter().all(|v| v.is_finite() && *v >= 0.0));
}

#[test]
fn without_a_measurement_pixels_pass_through() {
    let f = load(small_bands());
    let r = references().into_iter().next().unwrap();
    for (enc, orig) in [("linear", linear(&r.pixels)), ("srgb", srgb(&r.pixels))] {
        let mut img = orig.clone();
        run(&f, &mut img, &r, json!({"encoding": enc, "intensity": 150}));
        assert_eq!(img, orig, "{enc}");
        run(
            &f,
            &mut img,
            &r,
            json!({"encoding": enc, "_analysis": {"matrix": [1, 2, 3]}}),
        );
        assert_eq!(img, orig, "{enc}: malformed measurement");
    }
}

#[test]
fn degenerate_images_do_not_fail() {
    let never = AtomicBool::new(false);
    let f = load(Limits::default());
    // one pixel, all black, all white, NaNs and infinities
    for px in [
        vec![0.2f32, 0.3, 0.4],
        vec![0.0; 30],
        vec![1.0; 30],
        vec![f32::NAN, f32::INFINITY, -1.0, 0.5, 0.5, 0.5],
    ] {
        let (w, h) = ((px.len() / 3) as u32, 1);
        let a = f
            .analyze_rgb(&px, w, h, &json!({"encoding": "linear"}), &never)
            .unwrap()
            .unwrap();
        assert!(
            a["matrix"]
                .as_array()
                .unwrap()
                .iter()
                .all(|v| v.as_f64().unwrap().is_finite()),
            "{a}"
        );
        let mut img = px.clone();
        f.apply_rgb(
            &mut img,
            w,
            h,
            FrameInfo::whole(w, h),
            &json!({"encoding": "linear", "_analysis": a}),
            &never,
        )
        .unwrap();
    }
}
