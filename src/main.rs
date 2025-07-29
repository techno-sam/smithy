use std::{io::Read, sync::Arc};

use anvil::RegionFile;
use clap::Parser;
use fuser::MountOption;
use libc::{getegid, geteuid};
use log::{debug, error, info};
use smithy_fs::SmithyFS;
use cli::Cli;
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

    let args: Cli = Parser::parse();

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
