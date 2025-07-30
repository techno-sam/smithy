use std::{io::Read, sync::Arc};

use anvil::RegionFile;
use clap::{CommandFactory, Parser};
use clap_complete::{generate, generate_to};
use fuser::MountOption;
use libc::{getegid, geteuid};
use log::{debug, error, info};
use smithy_fs::SmithyFS;
use util::GuardedFile;

mod util;
mod smithy_fs;
mod cli;
mod anvil;

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default()
        .default_filter_or("info")
    ).init();

    let args: cli::Cli = Parser::parse();

    match args.command {
        cli::Command::Mount(args) => run_mount(args),
        cli::Command::Completion(args) => run_completion(args),
    }
}

fn run_mount(args: cli::MountCmd) {
    let mut options = vec![
        MountOption::NoAtime,
        MountOption::NoSuid,
        MountOption::NoDev,
        MountOption::NoExec,
        MountOption::DefaultPermissions,
        MountOption::FSName("smithy".to_string())
    ];

    if args.writable {
        options.push(MountOption::RW);
    } else {
        options.push(MountOption::RO);
    }

    if args.auto_unmount {
        options.push(MountOption::AutoUnmount);
    }

    let file = GuardedFile::new(&args.region_file.fname, args.writable).expect("Failed to find source file");
    let data = {
        let mut data = vec![];
        let read = file.get().read_to_end(&mut data).expect("Failed to read source file");
        debug!("Read {} bytes", read);
        data
    };
    let region = RegionFile::new(data);

    let uid = unsafe { geteuid() };
    let gid = unsafe { getegid() };

    info!("Exposing {} via FUSE at {}", args.region_file.fname, args.mount_point);

    let fs = SmithyFS::new(region, uid, gid, args.writable, file);
    let notif_mutex = Arc::clone(&fs.notifier);

    let mut session = match fuser::Session::new(fs, args.mount_point, &options) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to create FUSE session: {}", e);
            return;
        }
    };

    let notifier = session.notifier();

    {
        notif_mutex.lock().unwrap().replace(notifier);
    }

    session.run().unwrap();

    drop(session);

    info!("Unmounted cleanly");
}

fn run_completion(args: cli::CompletionCmd) {
    let bin_name = option_env!("CARGO_BIN_NAME").unwrap_or("smithy");
    let mut cmd = <cli::Cli as CommandFactory>::command();

    match args.out_dir {
        Some(out_dir) => {
            match generate_to(args.shell, &mut cmd, bin_name, out_dir) {
                Ok(path) => {
                    info!("Wrote completions file to: {}", path.display());
                }
                Err(err) => {
                    error!("Failed to write completions file: {}", err);
                }
            }
        }
        None => generate(args.shell, &mut cmd, bin_name, &mut std::io::stdout()),
    };
}
