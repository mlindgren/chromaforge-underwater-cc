// SPDX-License-Identifier: GPL-3.0-only
//
// Build tooling for underwater-cc.
//
//   cargo xtask build   builds the filter for wasm32-unknown-unknown (release) and writes
//                       dist/underwater-cc.wasm, after checking it has no imports and every
//                       export the host needs
//   cargo xtask check   only the checks, on dist/underwater-cc.wasm (or a path given)

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use wasmparser::{ExternalKind, Parser, Payload};

const REQUIRED: &[&str] = &[
    "memory",
    "pc_abi_version",
    "pc_manifest",
    "pc_alloc",
    "pc_filter",
    "pc_analyze",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// The module's imports and exports.
fn inspect(bytes: &[u8]) -> Result<(Vec<String>, Vec<String>), String> {
    let (mut imports, mut exports) = (Vec::new(), Vec::new());
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.map_err(|e| e.to_string())? {
            Payload::ImportSection(s) => {
                for imp in s {
                    let imp = imp.map_err(|e| e.to_string())?;
                    imports.push(format!("{}::{}", imp.module, imp.name));
                }
            }
            Payload::ExportSection(s) => {
                for e in s {
                    let e = e.map_err(|e| e.to_string())?;
                    if matches!(e.kind, ExternalKind::Func | ExternalKind::Memory) {
                        exports.push(e.name.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    Ok((imports, exports))
}

fn check(path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (imports, exports) = inspect(&bytes)?;
    if !imports.is_empty() {
        return Err(format!(
            "{} has imports (the host allows none): {imports:?}",
            path.display()
        ));
    }
    let missing: Vec<_> = REQUIRED
        .iter()
        .filter(|r| !exports.iter().any(|e| e == *r))
        .collect();
    if !missing.is_empty() {
        return Err(format!("{} lacks exports {missing:?}", path.display()));
    }
    println!(
        "{}: {} bytes, no imports, exports {exports:?}",
        path.display(),
        bytes.len()
    );
    Ok(())
}

fn build() -> Result<(), String> {
    let root = root();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .current_dir(&root)
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
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("cargo build failed (is the target installed? `rustup target add wasm32-unknown-unknown`)".into());
    }
    let built = root.join("target/wasm32-unknown-unknown/release/underwater_cc.wasm");
    let dist = root.join("dist");
    std::fs::create_dir_all(&dist).map_err(|e| e.to_string())?;
    let out = dist.join("underwater-cc.wasm");
    std::fs::copy(&built, &out).map_err(|e| format!("{}: {e}", built.display()))?;
    check(&out)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("build") => build(),
        Some("check") => check(
            &args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| root().join("dist/underwater-cc.wasm")),
        ),
        _ => Err("usage: cargo xtask build | check [path.wasm]".into()),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
