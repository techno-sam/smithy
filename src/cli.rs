use clap::{ArgAction, Parser, ValueHint};
use regex::Regex;

#[derive(Clone, Debug)]
pub struct ExtendedFilename {
    pub fname: String,
    pub x: isize,
    pub z: isize
}
impl ExtendedFilename {
    fn parse(s: &str) -> Result<Self, String> {
        let re = Regex::new(r"r\.(?P<x>-?\d+)\.(?P<z>-?\d+)\.mca$").unwrap();

        let caps = re.captures(s).ok_or(format!("`{}` must end with r.{{x}}.{{z}}.mca", s))?;

        let x = caps["x"].parse().map_err(|e| format!("x coordinate is not a number: {}", e))?;
        let z = caps["z"].parse().map_err(|e| format!("z coordinate is not a number: {}", e))?;

        Ok(Self {
            fname: s.to_owned(),
            x, z
        })
    }
}

#[derive(Parser)]
#[command(name = "Smithy")]
#[command(author)]
#[command(version)]
#[command(about)]
pub struct Cli {
    /// Region (Anvil) file to mount
    #[arg(value_hint=ValueHint::FilePath, value_parser=ExtendedFilename::parse)]
    pub region_file: ExtendedFilename,

    /// Path to mount the FUSE fs at
    #[arg(value_hint=ValueHint::DirPath)]
    pub mount_point: String,

    /// Allow writing
    #[arg(short, long)]
    #[arg(action=ArgAction::SetTrue)]
    pub writable: bool,

    /// Automatically unmount on process exit
    #[arg(short='u', long)]
    #[arg(action=ArgAction::SetTrue)]
    pub auto_unmount: bool,
}

