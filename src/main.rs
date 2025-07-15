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
        MountOption::RO,
        MountOption::FSName("smithy".to_string())
    ];

    if args.auto_unmount {
        options.push(MountOption::AutoUnmount);
    }

    let data = std::fs::read(&args.region_file).expect("Failed to write source file");
    let region = RegionFile::new(&data);

    let uid = unsafe { geteuid() };
    let gid = unsafe { getegid() };

    println!("Exposing {} via FUSE at {}", args.region_file, args.mount_point);

    fuser::mount2(SmithyFS::new(region, uid, gid), args.mount_point, &options).unwrap();

    println!("Unmounted cleanly");
}
