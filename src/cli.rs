/*
* Smithy
* Copyright (C) 2025  Sam Wagenaar
* This program is free software: you can redistribute it and/or modify
* it under the terms of the GNU Affero General Public License as published by
* the Free Software Foundation, either version 3 of the License, or
* (at your option) any later version.
* This program is distributed in the hope that it will be useful,
* but WITHOUT ANY WARRANTY; without even the implied warranty of
* MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
* GNU Affero General Public License for more details.
* You should have received a copy of the GNU Affero General Public License
* along with this program.  If not, see <http://www.gnu.org/licenses/>.
*/

use clap::{ArgAction, Parser, ValueHint, Subcommand, Args};
use clap_complete::Shell;
use regex::Regex;

#[allow(dead_code)]
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
#[command(name = "Smithy", bin_name="smithy")]
#[command(author)]
#[command(version)]
#[command(about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command
}

#[derive(Subcommand)]
pub enum Command {
    /// Mount a region file as a directory
    Mount(MountCmd),
    /// Generate shell completions
    Completion(CompletionCmd),
}

#[derive(Args)]
pub struct MountCmd {
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

#[derive(Args)]
pub struct CompletionCmd {
    #[arg(long, short)]
    #[arg(value_enum)]
    pub shell: Shell,

    /// Location to create completions script, or blank for stdout
    #[arg(long, short)]
    #[arg(value_hint=ValueHint::DirPath)]
    pub out_dir: Option<String>
}
