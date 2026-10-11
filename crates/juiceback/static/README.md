# cuelume.bundle.js
# Bundled from the cuelume npm package (same version vendored for juicefront
# under crates/juicefront/static/js/vendor/cuelume/).
# Regenerate with bun (no repo Node project needed):
#   printf import-shim > cuelume-entry.tmp.js (see below), then
#   bun build cuelume-entry.tmp.js --format=iife --minify --outfile=cuelume.bundle.js
# (or `bun x esbuild`, or `npx esbuild`, with the same flags)
# Shim content:
#   import { bind, play, setEnabled, setVolume, setTheme } from "cuelume";
#   window.Cuelume = { bind, play, setEnabled, setVolume, setTheme };
# (install the pinned cuelume version in a scratch dir first:
#  `bun add cuelume@<version>` — check the vendored copy's package.json)
# Shim content:
#   import { bind, play, setEnabled, setVolume, setTheme } from "cuelume";
#   window.Cuelume = { bind, play, setEnabled, setVolume, setTheme };
# Inlined into the password-unlock page (routes/unlock.rs), which has no static-file server.
# NOTE: bun 1.4 --global-name does not emit the global; the entry shim is required.
