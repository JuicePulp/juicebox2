#!/usr/bin/env bash
# Build the JuicePlayer bundle served by juicehost at /api/player/*.
#
# Pinned inputs:
#   JuicePlayer    ee1cf718892d159676779b722bd03376b4ea8164 (1.0.0 + main fixes)
#   Ruffle         0.7.1
#   @ffmpeg/core   0.12.10
#   GeneralUser GS 1.471 (8fe2798d527f1dfa2d58baf78ac7b1820a575703)
#
# Output: crates/juicehost/assets/juiceplayer/dist
# Deploy: copy that directory to <version>/static/juiceplayer
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd "$SCRIPT_DIR/../../.." && pwd)
OUT="$ROOT/crates/juicehost/assets/juiceplayer/dist"
WORK="${JUICE_WORKDIR:-/tmp/juiceplayer-build}"
ASSETS="${JUICE_ASSETS_WORKDIR:-/tmp/juiceplayer-assets}"

JUICE_REPO="${JUICE_REPO:-https://github.com/JuicePulp/juiceplayer.git}"
JUICE_COMMIT="${JUICE_COMMIT:-ee1cf718892d159676779b722bd03376b4ea8164}"
RUFFLE_VERSION="${RUFFLE_VERSION:-0.7.1}"
FFMPEG_CORE_VERSION="${FFMPEG_CORE_VERSION:-0.12.10}"
SOUNDFONT_COMMIT="${SOUNDFONT_COMMIT:-8fe2798d527f1dfa2d58baf78ac7b1820a575703}"
SOUNDFONT_URL="${SOUNDFONT_URL:-https://raw.githubusercontent.com/JustEnoughLinuxOS/generaluser-gs/$SOUNDFONT_COMMIT/GeneralUser%20GS%20v1.471.sf2}"
SOUNDFONT_LICENSE_URL="${SOUNDFONT_LICENSE_URL:-https://raw.githubusercontent.com/JustEnoughLinuxOS/generaluser-gs/$SOUNDFONT_COMMIT/LICENSE.txt}"

for cmd in git npm tar curl; do
  command -v "$cmd" >/dev/null || { echo "missing required command: $cmd" >&2; exit 1; }
done

rm -rf "$WORK"
git clone "$JUICE_REPO" "$WORK"
git -C "$WORK" checkout --detach "$JUICE_COMMIT"
(cd "$WORK" && npm ci --no-audit --no-fund && npm run build)

rm -rf "$OUT" "$ASSETS"
mkdir -p "$OUT/ruffle" "$OUT/licenses" "$ASSETS"

# Player element, lazy chunks and subtitle assets.
find "$WORK/packages/ui/dist" -maxdepth 1 -type f \
  \( -name '*.js' -o -name '*.wasm' -o -name '*.woff2' \) -exec cp {} "$OUT/" \;

# WebCodecs decoder worker plus its direct-decode assets. Do not copy the
# package's index.js: the UI bundle above already owns that filename.
for file in \
  decoder.worker.js audio.worklet.js software-decoder.worker.js \
  hevc-decode.js hevc-decode.wasm \
  libav-6.10.9.0-decoder-mpeg4.mjs \
  libav-6.10.9.0-decoder-mpeg4.wasm.mjs \
  libav-6.10.9.0-decoder-mpeg4.wasm.wasm; do
  cp "$WORK/packages/codek/dist/$file" "$OUT/"
done

# Flash (Ruffle) is self-hosted so the preview CSP does not need a CDN.
(cd "$ASSETS" && npm pack "@ruffle-rs/ruffle@$RUFFLE_VERSION" >/dev/null)
mkdir -p "$ASSETS/ruffle"
tar xzf "$ASSETS"/ruffle-rs-ruffle-*.tgz -C "$ASSETS/ruffle" --strip-components=1
find "$ASSETS/ruffle" -maxdepth 1 -type f \( -name 'ruffle.js' -o -name 'core.ruffle.*.js' -o -name '*.wasm' \) \
  -exec cp {} "$OUT/ruffle/" \;
cp "$ASSETS/ruffle/LICENSE_MIT" "$ASSETS/ruffle/LICENSE_APACHE" "$OUT/licenses/"

# FFmpeg compatibility fallback: the decode worker looks next to itself first.
(cd "$ASSETS" && npm pack "@ffmpeg/core@$FFMPEG_CORE_VERSION" >/dev/null)
mkdir -p "$ASSETS/ffmpeg"
tar xzf "$ASSETS"/ffmpeg-core-*.tgz -C "$ASSETS/ffmpeg" --strip-components=1
cp "$ASSETS/ffmpeg/dist/esm/ffmpeg-core.js" "$OUT/"
cp "$ASSETS/ffmpeg/dist/esm/ffmpeg-core.wasm" "$OUT/"

# General MIDI SoundFont for the MIDI path.
curl -fL --retry 3 -o "$OUT/gm.sf2" "$SOUNDFONT_URL"
curl -fL --retry 3 -o "$OUT/licenses/generaluser-LICENSE.txt" "$SOUNDFONT_LICENSE_URL"
SIZE=$(wc -c < "$OUT/gm.sf2")
if [ "$SIZE" -lt 1000000 ]; then
  echo "soundfont download looks too small ($SIZE bytes)" >&2
  exit 1
fi

cp "$WORK/COPYING" "$OUT/licenses/juiceplayer-COPYING"
cp "$WORK/LICENSE" "$OUT/licenses/juiceplayer-LICENSE"

cat > "$OUT/versions.txt" <<EOF
juiceplayer $JUICE_COMMIT
ruffle $RUFFLE_VERSION
@ffmpeg/core $FFMPEG_CORE_VERSION
GeneralUser GS v1.471 $SOUNDFONT_COMMIT
EOF

echo "JuicePlayer bundle written to $OUT"
du -sh "$OUT"
