mod client;
use clap::Parser;
fn main() -> std::process::ExitCode {
    match client::run(client::Cli::parse()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rawmakase-ctl: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
