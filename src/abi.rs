// SPDX-License-Identifier: GPL-3.0-only
//
// The PhotoCraft plug-in ABI v1 exports (and the v1.1 `pc_analyze` proposal) as Chromaforge's
// `lcc-filters` host calls them. The host instantiates the module afresh for every band, so
// blocks handed out by `pc_alloc` and the results of `pc_analyze` are simply never freed.
//
// Part of underwater-cc, a Rust port of underwater-image-color-correction by Nikolaj Bech Andersen
// (https://github.com/nikolajbech/underwater-image-color-correction). GPL-3.0-only; see LICENSE.

use crate::{ABI_VERSION, Fail, MANIFEST, check_shape, parse_params};

/// `len << 32 | ptr`, the way `pc_manifest` and `pc_analyze` return a block.
fn packed(ptr: *const u8, len: usize) -> i64 {
    (((len as u64) << 32) | (ptr as usize as u64 & 0xffff_ffff)) as i64
}

/// A 16-byte-aligned block of `n` bytes that lives as long as the instance (null: no memory).
fn alloc_block(n: usize) -> *mut u8 {
    let words = n.div_ceil(16).max(1);
    let mut v: Vec<u128> = Vec::new();
    if v.try_reserve_exact(words).is_err() {
        return core::ptr::null_mut();
    }
    v.resize(words, 0);
    let ptr = v.as_mut_ptr().cast::<u8>();
    core::mem::forget(v);
    ptr
}

/// The `len` bytes at `ptr` in linear memory (empty for a null or zero-length block).
///
/// # Safety
/// `ptr..ptr+len` must lie in linear memory, which holds for blocks from `pc_alloc`; the host
/// checks that before every call.
unsafe fn bytes<'a>(ptr: i32, len: i32) -> &'a [u8] {
    if ptr <= 0 || len <= 0 {
        return &[];
    }
    // SAFETY: see the function's contract.
    unsafe { core::slice::from_raw_parts(ptr as usize as *const u8, len as usize) }
}

#[unsafe(no_mangle)]
pub extern "C" fn pc_abi_version() -> i32 {
    ABI_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn pc_manifest() -> i64 {
    packed(MANIFEST.as_ptr(), MANIFEST.len())
}

#[unsafe(no_mangle)]
pub extern "C" fn pc_alloc(n: i32) -> i32 {
    if n < 0 {
        return 0;
    }
    alloc_block(n as usize) as usize as i32
}

/// The image at `buf` as `f32` samples, checked against the size and format.
///
/// # Safety
/// As [`bytes`].
unsafe fn samples<'a>(
    buf: i32,
    len: i32,
    w: i32,
    h: i32,
    channels: i32,
    format: i32,
) -> Result<&'a mut [f32], Fail> {
    let n = check_shape(len, w, h, channels, format)?;
    if n == 0 {
        return Ok(&mut []);
    }
    if buf <= 0 || buf % 4 != 0 {
        return Err(Fail::Memory);
    }
    // SAFETY: `buf` is a 4-aligned block of `len` = `n * 4` bytes in linear memory (contract).
    Ok(unsafe { core::slice::from_raw_parts_mut(buf as usize as *mut f32, n) })
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn pc_filter(
    buf: i32,
    len: i32,
    w: i32,
    h: i32,
    channels: i32,
    format: i32,
    params_ptr: i32,
    params_len: i32,
) -> i32 {
    // SAFETY: the host passes blocks it got from `pc_alloc` and filled (see `bytes`).
    let run = || -> Result<(), Fail> {
        let px = unsafe { samples(buf, len, w, h, channels, format) }?;
        let params = parse_params(unsafe { bytes(params_ptr, params_len) })?;
        crate::filter(px, &params);
        Ok(())
    };
    match run() {
        Ok(()) => 0,
        Err(f) => f as i32,
    }
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn pc_analyze(
    buf: i32,
    len: i32,
    w: i32,
    h: i32,
    channels: i32,
    format: i32,
    params_ptr: i32,
    params_len: i32,
) -> i64 {
    // SAFETY: as in `pc_filter`.
    let run = || -> Result<i64, Fail> {
        let px = unsafe { samples(buf, len, w, h, channels, format) }?;
        let params = parse_params(unsafe { bytes(params_ptr, params_len) })?;
        let Some(analysis) = crate::analyze(px, w as usize, h as usize, &params) else {
            return Ok(0);
        };
        let text = serde_json::to_vec(&analysis).map_err(|_| Fail::Memory)?;
        let block = alloc_block(text.len());
        if block.is_null() {
            return Err(Fail::Memory);
        }
        // SAFETY: `block` is a fresh allocation of at least `text.len()` bytes.
        unsafe { core::ptr::copy_nonoverlapping(text.as_ptr(), block, text.len()) };
        Ok(packed(block, text.len()))
    };
    run().unwrap_or(0)
}
