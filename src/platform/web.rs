//! Opening web pages in the default browser.
use anyhow::Result;
use std::process::Command;

pub(crate) fn open(url: &str) -> Result<()> {
    if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()?;
    } else if cfg!(windows) {
        Command::new("explorer").arg(url).spawn()?;
    } else {
        Command::new("xdg-open").arg(url).spawn()?;
    }
    Ok(())
}
