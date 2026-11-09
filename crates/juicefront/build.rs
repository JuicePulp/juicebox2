use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn git_head_path() -> Option<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    if let Some(dir) = std::process::Command::new("git")
        .args(["rev-parse", "--absolute-git-dir"])
        .current_dir(manifest_dir)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| PathBuf::from(text.trim().to_owned()))
        .filter(|dir| dir.is_dir())
    {
        let head = dir.join("HEAD");
        if head.is_file() {
            return Some(head);
        }
    }
    let fallback = manifest_dir.join("../../.git/HEAD");
    fallback.is_file().then_some(fallback)
}

fn hash_static_tree(dir: &Path) -> u64 {
    fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.filter_map(std::result::Result::ok) {
                let path = entry.path();
                if path.is_dir() {
                    collect(&path, out);
                } else if path.is_file() {
                    out.push(path);
                }
            }
        }
    }

    let mut files = Vec::new();
    collect(dir, &mut files);
    files.sort();

    let mut hash: u64 = 0xcbf29ce484222325;
    for path in files {
        let rel = path
            .strip_prefix(dir)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        for byte in rel.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        if let Ok(data) = fs::read(&path) {
            for byte in data {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
    }
    hash
}

fn main() {
    println!("cargo:rerun-if-changed=icons/");
    if let Some(head) = git_head_path() {
        println!("cargo:rerun-if-changed={}", head.display());
    }
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let dest = Path::new(&out_dir).join("icon_data.rs");

    let mut entries = Vec::new();
    let icons_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("icons");
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

    let static_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("static");
    println!("cargo:rerun-if-changed={}", static_dir.display());
    let package_version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_owned());
    println!(
        "cargo:rustc-env=JUICEFRONT_ASSET_VERSION={package_version}-{:016x}",
        hash_static_tree(&static_dir)
    );
}
