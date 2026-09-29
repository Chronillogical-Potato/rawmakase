fn main() {
    let macos = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    // Link directives are printed after the glue library is compiled: GNU ld
    // with --as-needed (the default linker on aarch64 Linux) drops a shared
    // library named before the static archive that uses it.
    let raw = pkg_config::Config::new()
        .atleast_version("0.22")
        .cargo_metadata(false)
        .env_metadata(true)
        .probe("libraw_r")
        .expect("Install libraw development headers (>= 0.22)");
    let cms = pkg_config::Config::new()
        .cargo_metadata(false)
        .env_metadata(true)
        .probe("lcms2")
        .expect("Install lcms2 development headers");
    let mut b = cc::Build::new();
    b.cpp(true).std("c++17").file("native/raw.cpp");
    if macos {
        println!("cargo:rerun-if-env-changed=LIBOMP_PREFIX");
        let prefix = std::env::var_os("LIBOMP_PREFIX")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                let output = std::process::Command::new("brew")
                    .args(["--prefix", "libomp"])
                    .output()
                    .ok()?;
                output.status.success().then(|| {
                    std::path::PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
                })
            })
            .expect("Install libomp with brew install libomp, or set LIBOMP_PREFIX");
        b.flag("-Xpreprocessor")
            .flag("-fopenmp")
            .include(prefix.join("include"));
        println!(
            "cargo:rustc-link-search=native={}",
            prefix.join("lib").display()
        );
    } else {
        b.flag("-fopenmp");
    }
    for p in raw.include_paths.iter().chain(cms.include_paths.iter()) {
        b.include(p);
    }
    b.compile("rawmakase_native");
    for lib in [&raw, &cms] {
        for path in &lib.link_paths {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
        for name in &lib.libs {
            // Homebrew LibRaw 0.22 advertises the removed GNU C++ runtime.
            let name = if macos && name == "stdc++" {
                "c++"
            } else {
                name
            };
            println!("cargo:rustc-link-lib={name}");
        }
    }
    println!(
        "cargo:rustc-link-lib={}",
        if macos { "omp" } else { "gomp" }
    );
    println!("cargo:rerun-if-changed=native/raw.cpp");
}
