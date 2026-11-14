# JuicePlayer assets

`juicehost` serves this bundle at `/api/player/*` and uses it to preview video,
audio, MIDI, tracker modules, Flash movies, GIFs and animated images.

The built files are not checked in. Rebuild them with:

```bash
crates/juicehost/scripts/build-juiceplayer.sh
```

Then deploy `crates/juicehost/assets/juiceplayer/dist` to
`<deployment>/static/juiceplayer` (next to `bin/juicehost`). At runtime the
directory can be overridden with `JUICEHOST_PLAYER_DIR`.

Pinned upstream inputs are documented in the script:

- [JuicePulp/juiceplayer](https://github.com/JuicePulp/juiceplayer) `5396d9a56516eb5f7c216fef9cb5073e5348c8a3`
  (GPL-3.0-or-later)
- [Ruffle](https://ruffle.rs) `0.7.1` (MIT / Apache-2.0), self-hosted
- [@ffmpeg/core](https://www.npmjs.com/package/@ffmpeg/core) `0.12.10`
  (GPL-2.0-or-later), self-hosted compatibility fallback
- [GeneralUser GS](https://www.schristiancollins.com/generaluser) v1.471,
  GeneralUser GS License v2.0, for MIDI rendering

License copies are emitted into `dist/licenses/`.
