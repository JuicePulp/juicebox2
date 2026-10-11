# cuelume.bundle.js
# Bundled from the cuelume npm package (same version vendored for juicefront).
# Regenerate from crates/juicefront/ui (needs its node_modules):
#   printf import-shim > cuelume-entry.tmp.js (see below), then
#   bun build cuelume-entry.tmp.js --format=iife --minify --outfile=../../../juiceback/static/cuelume.bundle.js
# Shim content:
#   import { bind, play, setEnabled, setVolume, setTheme } from "cuelume";
#   window.Cuelume = { bind, play, setEnabled, setVolume, setTheme };
# Inlined into the password-unlock page (routes/unlock.rs), which has no static-file server.
# NOTE: bun 1.4 --global-name does not emit the global; the entry shim is required.
