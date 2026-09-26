use std::{env, fs, path::Path};

fn main() {
    println!("cargo:rerun-if-changed=icons/");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let dest = Path::new(&out_dir).join("icon_data.rs");

    let mut entries = Vec::new();
    let mut icons_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("icons");
    if !icons_dir.exists() {
        icons_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/src/icons");
    }
    if let Ok(dir) = fs::read_dir(&icons_dir) {
        let mut names: Vec<String> = dir
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "svg"))
            .filter_map(|path| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(str::to_owned)
            })
            .collect();
        names.sort();
        for name in names {
            entries.push(format!(
                "{name:?} => Some(include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/icons/{name}.svg\"))),"
            ));
        }
    }

    let generated = format!(
        "pub fn get_raw(name: &str) -> Option<&'static str> {{\n    match name {{\n        {}\n        _ => None,\n    }}\n}}\n",
        entries.join("\n        ")
    );
    fs::write(&dest, generated).expect("failed to write icon_data.rs");

    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|output| output.trim().to_owned())
            .filter(|output| !output.is_empty())
            .unwrap_or_else(|| "unknown".to_owned())
    };
    println!(
        "cargo:rustc-env=JUICEFRONT_COMMIT_REF={}",
        git(&["rev-parse", "--abbrev-ref", "HEAD"])
    );
    println!(
        "cargo:rustc-env=JUICEFRONT_COMMIT_HASH={}",
        git(&["rev-parse", "HEAD"])
    );
}
