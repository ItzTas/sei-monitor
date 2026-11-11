use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::version::VERSION;

const USAGE_HINT: &str = "Run 'sei-monitor --help' for usage.";
const USAGE_ERROR: u8 = 2;

#[derive(Parser)]
#[command(
    name = "sei-monitor",
    version = VERSION,
    about = "Watch your processes on SEI and get told when something changes"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    #[command(about = "Serve the monitor-hub plugin protocol; the hub starts this command")]
    Plugin,
}

pub fn usage_exit(error: clap::Error) -> ExitCode {
    let _ = error.print();
    if !error.use_stderr() {
        return ExitCode::SUCCESS;
    }
    eprintln!("{USAGE_HINT}");
    ExitCode::from(USAGE_ERROR)
}
