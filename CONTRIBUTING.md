# Contributing to Juicebox
Welcome and thanks for helping out juicebox.

## Setup
To start working you must have a recent and proper Rust install.
If you plan on working or running the frontend, `bun` is preferred for the frontend 
however standard NodeJS installs can also work.

Copy `.env.example` to `.env` and fill in local-only secrets. Never EVER commit `.env` please.

then run the checks for whatever you touch:
  - `cargo fmt -p <crate> -- --check`
  - `cargo clippy -p <crate> --all-targets`
  - `cargo test -p <crate>`

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
