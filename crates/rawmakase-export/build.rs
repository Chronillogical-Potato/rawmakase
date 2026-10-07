//! Exports name the RAWmakase release that wrote them (`build_info::SOFTWARE`),
//! which is the app's version in the workspace's root manifest, not this crate's.
fn main() {
    // Read at run time: a build script compiled once can run for another checkout.
    let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("../../Cargo.toml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let text = std::fs::read_to_string(&manifest).expect("the workspace's root Cargo.toml");
    let version = text
        .split("\n[")
        .find(|section| section.starts_with("[package]") || section.starts_with("package]"))
        .and_then(|package| {
            package.lines().find_map(|line| {
                let value = line
                    .strip_prefix("version")?
                    .trim_start()
                    .strip_prefix('=')?;
                Some(value.trim().trim_matches('"').to_owned())
            })
        })
        .expect("a version in the root Cargo.toml's [package]");
    println!("cargo:rustc-env=RAWMAKASE_VERSION={version}");
}
