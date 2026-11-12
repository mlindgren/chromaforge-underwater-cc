# underwater-cc

Underwater color correction as a [Chromaforge](https://github.com/emmertex/chromaforge) pixel
filter: a WebAssembly module for the PhotoCraft plug-in ABI v1 (plus the v1.1 `pc_analyze`
proposal), written in Rust.

## Credits

This is a port of **[underwater-image-color-correction](https://github.com/nikolajbech/underwater-image-color-correction)**
by **Nikolaj Bech Andersen** (`index.js`, version 1.0.3, commit `b366f32`), which is licensed under
the GPL-3.0. The algorithm, its constants and its quirks are his; this repository translates it to
Rust (`src/algo.rs`) and adapts it to Chromaforge's filter host. The example images in
`tests/fixtures/example*.jpg` come from the same repository (left half: original; right half: his
corrected result).

## License

GPL-3.0-only, as a derivative of a GPL-3.0 work. See [LICENSE](LICENSE).

## What it does

The original algorithm, run once per photo (`pc_analyze`):

1. average color of the image;
2. hue-shift red in (rotating green/blue into the red channel) until the average red reaches the
   target (default 60 of 255; at most 120°);
3. histograms of the new red, green and blue;
4. per channel, the widest run of levels between "empty" bins (count < pixels/2000 + 2);
5. a 4×5 color matrix that stretches that run to the full range, with the red row mixing in green
   and blue by the hue shift.

The measurement is stored with the photo as `_analysis`:

```json
{"version": 1, "matrix": [20 numbers], "hueShift": 47, "averageColor": [r, g, b], "minAvgRed": 60}
```

Every render (`pc_filter`) then applies `M' = I + k (M − I)` with `k = intensity / 100`, the
way the original README applies the matrix:

```
R' = rr·R + rg·G + rb·B + ro      G' = gg·G + go      B' = bb·B + bo
```

on sRGB-encoded 0..1 values (the matrix's offsets are already fractions of full scale: the
original adds `offset × 255` to 0..255 values). Without a measurement the pixels pass through
unchanged.

| `encoding` | what happens |
| --- | --- |
| `srgb` (Display stage) | applied directly, clamped to 0..1 like the original's canvas |
| `linear` (Source stage, HDR renders) | encoded to sRGB, applied, decoded back; negatives clamp to 0, values above 1 are kept |

The measurement converts the host's linear 1024 px copy to 8-bit sRGB code values, exactly what
the original reads from a canvas, so it gives the original's matrix to within 1e-9.

### Parameters

| name | type | default | |
| --- | --- | --- | --- |
| `intensity` | number 0..150 | 100 | 0 = off, 100 = the original's correction, above 100 extrapolates |
| `minAvgRed` | number 0..128 | 60 | the hue-shift target (the original's `minAvgRed`); it is read when measuring, so **measure again after changing it** |

The manifest asks for the **Display** stage (`"stage": "display"`), `overlap: 0`.

## Build

```bash
rustup target add wasm32-unknown-unknown
cargo xtask build
```

writes `dist/underwater-cc.wasm` (about 85 KB; release, `opt-level = "s"`, LTO, `panic = "abort"`)
and checks it has **no imports** and all the exports the host needs. `cargo xtask check [file]`
runs only the check.

## Install in Chromaforge

In the app: File ▸ Plug-ins ▸ Plug-in Manager… ▸ install `underwater-cc.wasm`. From the CLI:

```bash
chromaforge-cli run plugin.install path=/path/to/underwater-cc.wasm
```

Then, per photo: Develop ▸ Plug-in Filters ▸ add *Underwater Color Correction*, then **Measure**.
The same from the CLI or MCP:

```
develop.pluginFilter.add {"plugin": "underwater-color-correction"}          → {uid}
develop.pluginFilter.analyze {"uid": 1, "wait": true}
develop.pluginFilter.set {"uid": 1, "params": {"intensity": 80}}
```

Batch: `develop.pluginFilter.analyze {"uid": 1, "ids": [...]}` measures each photo on its own.

## Tests

```bash
cargo test
```

- `src/`: unit tests (shape checks, params, the sRGB curves and tables, degenerate images).
- `tests/host.rs`: builds the .wasm, loads it with Chromaforge's own host (`lcc-filters`, a git
  dev-dependency) and checks the manifest, the empty import section, `pc_analyze` against the
  original's matrices for `example1-3`, intensity 0 = identity and intensity 100 = the original's
  output in both encodings, and that small bands (dozens of fresh instances per image) give the
  same result.

The fixtures (`tests/fixtures/*.png`, `reference.json`) are made by the **original JavaScript**:
`tools/reference/make-reference.js` decodes each example JPEG with jpeg-js, stores the pixels
losslessly, runs `index.js` and the README's filtering code, and records the matrices:

```bash
git clone https://github.com/nikolajbech/underwater-image-color-correction ../uw-original
(cd tools/reference && npm ci)
node tools/reference/make-reference.js ../uw-original
```

`cargo run --release --example bench` (after `cargo xtask build`) times the module in the host.

`tools/e2e.sh /path/to/chromaforge-cli OUT_DIR` runs the whole flow with the real CLI (install,
add at each stage, measure with `wait: true`, export) on the example images (needs Python + Pillow
to crop them).

## Which stage?

On JPEGs the two stages give practically the same picture (the measurement is taken on the
same pixels either way), and Display is several times cheaper per render (no curve
conversions). Display is the default. Note the measurement is always taken on the source frame,
before any develop adjustment: after big White Balance or Exposure changes in the Display stage,
measure again or use the Source stage, where measurement and filter see the same pixels.
