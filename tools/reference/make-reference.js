// SPDX-License-Identifier: GPL-3.0-only
//
// Generates the reference fixtures for the parity tests by running the ORIGINAL JavaScript
// algorithm (index.js of https://github.com/nikolajbech/underwater-image-color-correction,
// by Nikolaj Bech Andersen, GPL-3.0) on its own example images.
//
// Usage (from the repo root):
//   git clone https://github.com/nikolajbech/underwater-image-color-correction ../uw-original
//   (cd tools/reference && npm ci)
//   node tools/reference/make-reference.js ../uw-original
//
// Writes into tests/fixtures/:
//   exampleN.png              the JPEG decoded (by jpeg-js) to 8-bit RGB, stored losslessly so
//                             the Rust tests see exactly the pixels the JS saw
//   exampleN.js-filtered.png  the image filtered with the README's reference formula
//                             (written into a Uint8ClampedArray, as a canvas would)
//   reference.json            each image's color filter matrix and the upstream commit

'use strict'

const fs = require('fs')
const path = require('path')
const { execSync } = require('child_process')
const jpeg = require('jpeg-js')
const { PNG } = require('pngjs')

const upstream = path.resolve(process.argv[2] || '../uw-original')
const getColorFilterMatrix = require(path.join(upstream, 'index.js'))
const out = path.resolve(__dirname, '..', '..', 'tests', 'fixtures')

let commit = 'unknown'
try {
    commit = execSync('git rev-parse HEAD', { cwd: upstream }).toString().trim()
} catch (_) { /* not a git checkout */ }

function writePng(file, rgba, width, height) {
    const png = new PNG({ width, height, colorType: 2, inputColorType: 6 })
    Buffer.from(rgba.buffer, rgba.byteOffset, rgba.length).copy(png.data)
    fs.writeFileSync(file, PNG.sync.write(png, { colorType: 2, inputColorType: 6 }))
}

const images = []
for (const name of ['example1', 'example2', 'example3']) {
    const raw = jpeg.decode(fs.readFileSync(path.join(upstream, name + '.jpg')), {
        useTArray: true,
        formatAsRGBA: true,
    })
    const { width, height } = raw
    const pixels = new Uint8ClampedArray(raw.data.buffer, raw.data.byteOffset, raw.data.length)
    const matrix = getColorFilterMatrix(pixels, width, height)

    // The README's way of applying the matrix to canvas (RGBA 0..255) data.
    const data = new Uint8ClampedArray(pixels)
    const filter = matrix
    for (let i = 0; i < data.length; i += 4) {
        data[i] = Math.min(255, Math.max(0, data[i] * filter[0] + data[i + 1] * filter[1] + data[i + 2] * filter[2] + filter[4] * 255)) // Red
        data[i + 1] = Math.min(255, Math.max(0, data[i + 1] * filter[6] + filter[9] * 255)) // Green
        data[i + 2] = Math.min(255, Math.max(0, data[i + 2] * filter[12] + filter[14] * 255)) // Blue
    }

    writePng(path.join(out, name + '.png'), pixels, width, height)
    writePng(path.join(out, name + '.js-filtered.png'), data, width, height)
    images.push({ name, width, height, matrix })
    console.log(name, width + 'x' + height, JSON.stringify(matrix.map(v => +v.toFixed(5))))
}

fs.writeFileSync(
    path.join(out, 'reference.json'),
    JSON.stringify({ upstream: 'https://github.com/nikolajbech/underwater-image-color-correction', commit, images }, null, 2) + '\n',
)
