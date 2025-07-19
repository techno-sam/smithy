use std::sync::Arc;

use anvil::RegionFile;
use clap::Parser;
use fuser::MountOption;
use libc::{getegid, geteuid};
use smithy_fs::SmithyFS;
use cli::Cli;

mod smithy_fs;
mod cli;
mod anvil;

fn main() {
    env_logger::init();
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

    let data = std::fs::read(&args.region_file.fname).expect("Failed to write source file");
    let region = RegionFile::new(data);

    let uid = unsafe { geteuid() };
    let gid = unsafe { getegid() };

    println!("Exposing {} via FUSE at {}", args.region_file.fname, args.mount_point);

    let fs = SmithyFS::new(region, uid, gid, args.writable);
    let notif_mutex = Arc::clone(&fs.notifier);

    let mut session = fuser::Session::new(fs, args.mount_point, &options).unwrap();
    let notifier = session.notifier();

    {
        notif_mutex.lock().unwrap().replace(notifier);
    }

    session.run().unwrap();

    println!("Unmounted cleanly");
}
