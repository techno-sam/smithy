use std::time::{Duration, SystemTime, UNIX_EPOCH};
use fuser::{FileAttr, FileType, Filesystem, FUSE_ROOT_ID};
use libc::ENOENT;
use log::debug;

use crate::anvil::RegionFile;


const TTL: Duration = Duration::from_secs(1);
const ROOT_DIR_ATTR: FileAttr = fattr(FUSE_ROOT_ID, 0, UNIX_EPOCH, FileType::Directory, 0o555, 2, 0, 0);


const fn fattr(ino: u64, size: u64, time: SystemTime, kind: FileType, perm: u16, nlink: u32, uid: u32, gid: u32) -> FileAttr {
    let blksize: u32 = 512;
    let blocks = size.div_ceil(blksize as u64);

    FileAttr {
        ino,
        size,
        blocks,
        atime: time,
        mtime: time,
        ctime: time,
        crtime: time,
        kind,
        perm,
        nlink,
        uid,
        gid,
        rdev: 0,
        blksize,
        flags: 0
    }
}


fn parse_file_name(name: &str) -> Option<(u8, u8)> {
    enum FSM {
        Uninit,
        X{x: u8, n: u8},
        Z{x: u8, z: u8, n: u8},
    }
    use FSM::*;

    if !name.ends_with(".nbt") {
        return None;
    }
    let name = &name[0..name.len()-4];

    let mut chars = name.chars();
    let mut state = Uninit;

    while let Some(c) = chars.next() {
        state = match state {
            Uninit => {
                match c {
                    'x' => X { x: 0, n: 2 },
                    _ => break
                }
            }
            X{x, n} => {
                if let Some(d) = c.to_digit(10) {
                    if n == 0 {
                        break
                    }

                    if n < 2 && x == 0 {
                        break
                    }

                    X { x: x * 10 + (d as u8), n: n - 1 }
                } else if c == 'z' {
                    Z { x, z: 0, n: 2 }
                } else {
                    break
                }
            }
            Z{x, z, n} => {
                if n == 0 {
                    return None
                }

                if n < 2 && z == 0 {
                    return None
                }

                if let Some(d) = c.to_digit(10) {
                    Z { x, z: z * 10 + (d as u8), n: n - 1 }
                } else {
                    return None
                }
            }
        };
    }

    match state {
        Z{x, z, n} if n < 2 => Some((x, z)),
        _ => None
    }
}


const MIN_COORD_INODE: u64 = coords_to_inode(0, 0);
const MAX_COORD_INODE: u64 = coords_to_inode(31, 31);

#[inline(always)]
const fn coords_to_inode(x: u8, z: u8) -> u64 {
    let x = (x & 31) as u64;
    let z = (z & 31) as u64;

    return FUSE_ROOT_ID + 1 + (x + z*32);
}

#[inline(always)]
fn inode_to_coords(inode: u64) -> Option<(u8, u8)> {
    if MIN_COORD_INODE <= inode && inode <= MAX_COORD_INODE {
        let packed = inode - 1 - FUSE_ROOT_ID;
        let x = (packed & 31) as u8;
        let z = ((packed >> 5) & 31) as u8;
        Some((x, z))
    } else {
        None
    }
}


pub(crate) struct SmithyFS<'a> {
    region: RegionFile<'a>,
    uid: u32,
    gid: u32,
    root_dir_attr: FileAttr
}

impl<'a> SmithyFS<'a> {
    pub(crate) fn new(region: RegionFile<'a>, uid: u32, gid: u32) -> Self {
        Self {
            region,
            uid,
            gid,
            root_dir_attr: FileAttr {
                uid,
                gid,
                ..ROOT_DIR_ATTR
            }
        }
    }

    fn chunk_attr(&self, x: u8, z: u8) -> FileAttr {
        let chunk = self.region.lookup_chunk(x, z);
        let time = self.region.lookup_timestamp(x, z);

        fattr(coords_to_inode(x, z), chunk.data.len() as u64, time, FileType::RegularFile, 0o444, 1, self.uid, self.gid)
    }
}

impl Filesystem for SmithyFS<'_> {
    fn lookup(&mut self, _req: &fuser::Request<'_>, parent: u64, name: &std::ffi::OsStr, reply: fuser::ReplyEntry) {
        if parent != FUSE_ROOT_ID {
            reply.error(ENOENT);
            return;
        }

        debug!("Lookup in {} of {:?}", parent, name);

        if let Some((x, z)) = name.to_str().and_then(parse_file_name) {
            debug!("Parsed file name as chunk ({}, {})", x, z);
            if x < 32 && z < 32 {
                reply.entry(&TTL, &self.chunk_attr(x, z), 0);
                return;
            }
        }

        debug!("Failed to look up file {:?} in {}", name, parent);
        reply.error(ENOENT);
    }

    fn getattr(&mut self, _req: &fuser::Request<'_>, ino: u64, _fh: Option<u64>, reply: fuser::ReplyAttr) {
        if ino == FUSE_ROOT_ID {
            reply.attr(&TTL, &self.root_dir_attr);
        } else if let Some((x, z)) = inode_to_coords(ino) {
            reply.attr(&TTL, &self.chunk_attr(x, z));
        } else {
            reply.error(ENOENT);
        }
    }

    fn read(
            &mut self,
            _req: &fuser::Request<'_>,
            ino: u64,
            _fh: u64,
            offset: i64,
            size: u32,
            _flags: i32,
            _lock_owner: Option<u64>,
            reply: fuser::ReplyData,
        ) {
        if let Some((x, z)) = inode_to_coords(ino) {
            let chunk = self.region.lookup_chunk(x, z).data;

            let offset = offset as usize;
            let size = size as usize;

            if offset >= chunk.len() {
                reply.data(&[]);
            } else {
                let end = (offset + size).min(chunk.len());
                reply.data(&chunk[offset..end]);
            }
        } else {
            reply.error(ENOENT)
        }
    }

    fn readdir(
            &mut self,
            _req: &fuser::Request<'_>,
            ino: u64,
            _fh: u64,
            offset: i64,
            mut reply: fuser::ReplyDirectory,
        ) {
        if ino != 1 {
            reply.error(ENOENT);
            return;
        }

        // '.', '..', and the nbt files
        let count = 2 + (31 + 31 * 32);

        for i in (offset as u64)..count {
            let (inode, file_type, name) = match i {
                0 => (FUSE_ROOT_ID, FileType::Directory, "."),
                1 => (FUSE_ROOT_ID, FileType::Directory, ".."),
                packed => {
                    let packed = packed - 2;
                    let x = (packed & 31) as u8;
                    let z = ((packed >> 5) & 31) as u8;
                    (coords_to_inode(x, z), FileType::RegularFile, &format!("x{x}z{z}.nbt") as &str)
                }
            };

            if reply.add(inode, (i + 1) as i64, file_type, name) {
                break;
            }
        }

        reply.ok();
    }
}
