#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
# End to end with the Chromaforge CLI: install the module, add the filter at each stage, measure
# (wait: true) and export, for each example image's left (uncorrected) half.
#   tools/e2e.sh /path/to/chromaforge-cli OUT_DIR
# Uses a throwaway HOME and plug-in folder, so the real app's settings are untouched.
set -euo pipefail
CLI=$1; OUT=$(mkdir -p "$2" && cd "$2" && pwd)
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WASM=$ROOT/dist/underwater-cc.wasm
export HOME=$OUT/home CHROMAFORGE_PLUGINS=$OUT/plugins
mkdir -p "$HOME" "$CHROMAFORGE_PLUGINS" "$OUT/in"
"$CLI" run plugin.install path="$WASM" >/dev/null
for n in 1 2 3; do
  python3 -I -c "import sys; from PIL import Image; im=Image.open(sys.argv[1]).convert('RGB'); w,h=im.size; im.crop((0,0,w//2,h)).save(sys.argv[2], quality=95)" \
    "$ROOT/tests/fixtures/example$n.jpg" "$OUT/in/uw$n.jpg"
  "$CLI" run --import "$OUT/in/uw$n.jpg" app.export path="$OUT/uw$n-before.jpg" >/dev/null
  for stage in display source; do
    "$CLI" run --import "$OUT/in/uw$n.jpg" \
      develop.pluginFilter.add plugin=underwater-color-correction stage=$stage \
      develop.pluginFilter.analyze uid=1 wait=true \
      app.export path="$OUT/uw$n-$stage.jpg" > "$OUT/uw$n-$stage.log"
  done
done
echo "renders in $OUT"
