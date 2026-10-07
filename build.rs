fn git_output(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn mcp_guide() {
    println!("cargo:rerun-if-changed=docs/mcp.md");
    // Track both detached HEADs and branch updates, including linked worktrees.
    for name in [
        Some("HEAD".to_owned()),
        git_output(&["symbolic-ref", "-q", "HEAD"]),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(path) = git_output(&["rev-parse", "--git-path", &name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    // Link to the last committed guide revision, never a feature branch that
    // may disappear. Source archives without Git use their release tag.
    let revision = git_output(&["log", "-1", "--format=%H", "--", "docs/mcp.md"])
        .filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or_else(|| format!("v{}", std::env::var("CARGO_PKG_VERSION").unwrap()));
    println!(
        "cargo:rustc-env=RAWMAKASE_MCP_GUIDE=https://github.com/pch/rawmakase/blob/{revision}/docs/mcp.md"
    );
}

fn main() {
    mcp_guide();
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=packaging/windows/rawmakase.ico");
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("packaging/windows/rawmakase.ico")
            .set("ProductName", "RAWmakase")
            .set("FileDescription", "RAWmakase");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=Windows resources not embedded: {error}");
        }
    }
}
