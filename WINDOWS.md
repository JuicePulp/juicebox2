# Running juicebox² on Windows

The whole workspace compiles for 64-bit Windows (10 or newer). The services are
ordinary console programs; there is no installer and no GUI.

## Requirements

- Rust 1.88+ with the 2024 edition (the repo's `rust-version`).
- Either the MSVC toolchain (Visual Studio Build Tools with "Desktop development
  with C++") or a MinGW-w64 toolchain for the `x86_64-pc-windows-gnu` target.
- Optional, only for media fetching/embedding: `ffmpeg`, `ffprobe`, `yt-dlp`,
  `deno`. Put them on `PATH` or point the config keys at them.

## Build

From the repository root:

```powershell
cargo build --release -p juiceback -p juicehost -p juicefront -p juicerouter
```

Add `-p juicebox` if you also want the orchestrator, which builds and supervises
the three main services for you. With the GNU toolchain, pass
`--target x86_64-pc-windows-gnu`; the binaries land in
`target\<target>\release\`.

To verify a Windows build from another OS without a linker:

```sh
cargo check --target x86_64-pc-windows-gnu --workspace --all-targets
```

## Run

Copy the example configs next to the binaries (or anywhere, via the env vars):

```powershell
Copy-Item juiceback.toml.example juiceback.toml
Copy-Item juicehost.toml.example juicehost.toml
Copy-Item juicefront.toml.example juicefront.toml
$env:JWT_SECRET = "replace-with-a-real-secret"
.\juiceback.exe
```

Start `.\juicehost.exe` and `.\juicefront.exe` the same way, in separate shells.
`JUICEBACK_CONFIG`, `JUICEHOST_CONFIG` and `JUICEFRONT_CONFIG` can point to
config files outside the working directory. Relative paths inside the TOML
files (e.g. `./files`, `./delta.db`) are resolved against the working directory,
so start all three from the same folder.

Alternatively, from `target\release\`, run `juicebox.exe`. It compiles the
services on first launch, starts them as child processes, restarts crash loops
and adds the `.exe` suffix itself. Set `JUICEBOX_NO_BUILD=1` to use already
compiled binaries.

## Windows-specific behaviour

- There are no Unix signals. Services stop on Ctrl+C or console close. The
  orchestrator terminates children with `TerminateProcess`, so they do not get
  the graceful drain that `SIGTERM` provides on Linux.
- Config lookup checks the working directory, the `*CONFIG` env var, and
  `%ProgramData%\juicebox\` instead of `/etc/juicebox`.
- HTTP/3 (QUIC) is UDP-based; allow UDP 6402/6403 through the firewall. If you
  do not need it, build `juiceback` and `juicehost` with
  `--no-default-features` to drop QUIC and use plain HTTP.
- Multipart concatenation falls back to seek-based positional I/O: it is
  functionally identical to Linux but skips the `copy_file_range`/reflink
  fast paths.
- The generated QUIC private key is written with the default Windows ACL.
  Restrict its directory with `icacls` if other users share the machine.
- Use forward slashes or escaped backslashes for paths in TOML files.
