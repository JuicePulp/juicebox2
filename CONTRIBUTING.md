# Contributing to Juicebox
Welcome and thanks for helping out juicebox.

## Setup
To start working you must have a recent and proper Rust install.

Copy `.env.example` to `.env` and fill in local-only secrets. Never EVER commit `.env` please.

then run the checks for whatever you touch:
  - `cargo fmt -p <crate> -- --check`
  - `cargo clippy -p <crate> --all-targets`
  - `cargo test -p <crate>`

## Frontend development (juicefront)
The frontend is Rust (Axum + Askama templates + vanilla JS in
`crates/juicefront/static/js/`). There is no Node/bundler step anymore. 

You can do a very rough live reload system while developing:

  cargo watch -w crates/juicefront -x 'run -p juicefront'

Saving any `.rs` file, template, `static/` asset, or `i18n/` dict rebuilds and restarts the server + browser reloads automatically;

Live reload is on by default for dev build, you can override it by toggling (0/1) the `JUICEFRONT_LIVE=1` env var.

Running the full stack via `cargo run` (the `juicebox` supervisor) also
watches workspace sources (`crates/*/src`, `templates/`, `static/`,
`i18n/`, `icons/`, manifests): saving a file rebuilds and gracefully
restarts all three services, and browsers reload on their own through the
same live channel. Set `JUICEBOX_WATCH=0` to disable it, or
`JUICEBOX_NO_BUILD=1` to skip builds entirely. Changing secrets in `.env`
still needs a full `cargo run` restart (respawned services inherit the
supervisor's original environment). 

## Pull requests

- Keep diffs small and focused and should have a single concern per PR if possible.
- Add or update tests for behavior changes, pure helpers need unit tests!!
- Do not include generated output like `target/`, `public/rustdoc/`, or the lockfile unrelated to your change
  > Usually .gitignore should take care of this, but if not, please do not commit it.
- Write commit messages that say what changed and why if possible; just try your best as I (juiceydev) usually don't do that.

## Policy on AI-Generated Code

We allow the use of AI tools (like LLMs or coding agents) to write code **however** a qualified and skilled human developer must review and take full responsibility for it.
That means ABSOLUTELY NO AI AGENTS ARE ALLOWED.

Write it yourself if you can please.. and AI-generated code WILL receive stricter reviews and takes longer to be merged.

Be honest about AI use, always put in your PR description if you used an AI tool, which one you used, and what files it created, Hidden AI code will be rejected immediately and you will probably be banned from the project.

Understand your code. You are responsible for every line and you must be able to explain how the code works. "The AI wrote it" is ***NOT*** an acceptable answer.
