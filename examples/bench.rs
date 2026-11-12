// SPDX-License-Identifier: GPL-3.0-only
//! Times the module in the real host: `cargo xtask build && cargo run --release --example bench`.
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use lcc_filters::{Filter, FrameInfo, Limits};
use serde_json::json;

fn main() {
    let wasm = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/dist/underwater-cc.wasm"
    ))
    .expect("run `cargo xtask build` first");
    let f = Filter::load(&wasm, Limits::default()).expect("load");
    let never = AtomicBool::new(false);
    let (w, h) = (4000u32, 3000u32);
    let img: Vec<f32> = (0..(w * h * 3) as usize)
        .map(|i| ((i * 7919) % 1000) as f32 / 1000.0)
        .collect();
    let (aw, ah) = (1024u32, 768u32);
    let small: Vec<f32> = img[..(aw * ah * 3) as usize].to_vec();
    let t = Instant::now();
    let a = f
        .analyze_rgb(&small, aw, ah, &json!({"encoding": "linear"}), &never)
        .unwrap()
        .unwrap();
    println!("analyze 1024x768: {:?}", t.elapsed());
    for enc in ["srgb", "linear"] {
        let mut buf = img.clone();
        let t = Instant::now();
        f.apply_rgb(
            &mut buf,
            w,
            h,
            FrameInfo::whole(w, h),
            &json!({"encoding": enc, "_analysis": a}),
            &never,
        )
        .unwrap();
        println!(
            "filter 12 MP {enc}: {:?} ({} threads)",
            t.elapsed(),
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        );
    }
}
