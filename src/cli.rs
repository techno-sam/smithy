use clap::{ArgAction, Parser, ValueHint};

#[derive(Parser)]
#[command(name = "Smithy")]
#[command(author)]
#[command(version)]
#[command(about)]
pub struct Cli {
    /// Region (Anvil) file to mount
    #[arg(value_hint=ValueHint::FilePath)]
    pub region_file: String,

    /// Path to mount the FUSE fs at
    #[arg(value_hint=ValueHint::DirPath)]
    pub mount_point: String,

    /// Automatically unmount on process exit
    #[arg(short='u', long)]
    #[arg(action=ArgAction::SetTrue)]
    pub auto_unmount: bool
}

