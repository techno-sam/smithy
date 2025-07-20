use std::{collections::HashMap, sync::{Arc, Mutex}, time::{Duration, SystemTime, UNIX_EPOCH}};
use fuser::{FileAttr, FileType, Filesystem, Notifier, FUSE_ROOT_ID};
use int_enum::IntEnum;
use libc::{EACCES, EBADF, EEXIST, EFBIG, EINVAL, ENOENT, ENOSYS, ENOTDIR, EPERM, EROFS};
use log::{debug, error, info, warn};

use crate::anvil::{Chunk, CompressionType, RegionFile, MAX_CHUNK_LEN, SECTOR_LEN};


const TTL: Duration = Duration::from_secs(1);
const ROOT_DIR_ATTR: FileAttr = fattr(FUSE_ROOT_ID, 0, UNIX_EPOCH, FileType::Directory, 0o555, 2, 0, 0);


const fn fattr(ino: u64, size: u64, time: SystemTime, kind: FileType, perm: u16, nlink: u32, uid: u32, gid: u32) -> FileAttr {
    FileAttr {
        ino,
        size,
        // blocks are semi-standardized as 512-byte units, according to man inode.7
        blocks: size.div_ceil(512),
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
        // preferred IO size, not related to `blocks`
        blksize: SECTOR_LEN as u32,
        flags: 0
    }
}


#[derive(Clone, Copy, Debug)]
struct FileKey {
    /// Must be < 32
    x: u8,
    /// Must be < 32
    z: u8,
    kind: FileKind
}

impl FileKey {
    fn parse(name: &str) -> Option<Self> {
        enum FSM {
            Uninit,
            X{x: u8, n: u8},
            Z{x: u8, z: u8, n: u8},
        }
        use FSM::*;

        let (kind, name) = FileKind::parse_extension(name)?;

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
            Z{x, z, n} if n < 2 && x < 32 && z < 32 => Some(Self { x, z, kind }),
            _ => None
        }
    }
}


/*fn make_compression_info(chunk: &Chunk<'_>) -> String {
    return format!(
        "# The value in this file MUST match the actual compression of {}\n{}\n",
        FileKind::Chunk.make_fname(chunk.x, chunk.z),
        chunk.compression_type.make_selector_string()
    );
}*/


#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, IntEnum)]
enum FileKind {
    Chunk = 0,
    CompressionInfo  = 1
}
impl FileKind {
    fn make_fname(self, x: u8, z: u8) -> String {
        let ext = match self {
            Self::Chunk => ".nbt",
            Self::CompressionInfo => ".cmp",
        };

        format!("x{}z{}{}", x, z, ext)
    }

    fn parse_extension(fname: &str) -> Option<(Self, &str)> {
        if fname.len() < 4 {
            return None;
        }

        match &fname[fname.len()-4..] {
            ".nbt" => Some((Self::Chunk, &fname[0..fname.len()-4])),
            ".cmp" => Some((Self::CompressionInfo, &fname[0..fname.len()-4])),
            _ => None
        }
    }

    fn is_chunk(self) -> bool {
        match self {
            FileKind::Chunk => true,
            _ => false
        }
    }
}

struct FileHandle {
    perms: u8
}
impl FileHandle {
    fn new(read: bool, write: bool) -> Self {
        let perms = (read as u8) | ((write as u8) << 1);
        Self { perms }
    }

    #[inline(always)]
    fn can_read(&self) -> bool {
        self.perms & 1 != 0
    }

    #[inline(always)]
    fn can_write(&self) -> bool {
        self.perms & 2 != 0
    }
}

struct FileHandleAlloc(u64);
impl FileHandleAlloc {
    fn new() -> Self {
        Self(0)
    }

    fn alloc(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }
}

fn read_into(data: &[u8], offset: usize, size: usize, reply: fuser::ReplyData) {
    if offset >= data.len() {
        reply.data(&[]);
    } else {
        let end = (offset + size).min(data.len());
        reply.data(&data[offset..end]);
    }
}

enum InodeData {
    Chunk(Vec<u8>),
    Info(CompressionType),
}
impl InodeData {
    fn new(kind: FileKind, chunk: &Chunk<'_>) -> InodeData {
        match kind {
            FileKind::Chunk => InodeData::Chunk(chunk.data.to_owned()),
            FileKind::CompressionInfo => InodeData::Info(chunk.compression_type),
        }
    }

    fn blank(kind: FileKind) -> Self {
        match kind {
            FileKind::Chunk => InodeData::Chunk(vec![]),
            FileKind::CompressionInfo => InodeData::Info(CompressionType::Unknown(42)),
        }
    }

    fn len(&self) -> usize {
        match self {
            InodeData::Chunk(data) => data.len(),
            InodeData::Info(ct) => ct.make_selector_string().len(),
        }
    }

    fn read(&self, offset: i64, size: u32, reply: fuser::ReplyData) {
        if offset < 0 {
            reply.error(EINVAL);
            return;
        }

        let offset = offset as usize;
        let size = size as usize;

        match self {
            Self::Chunk(chunk) => {
                read_into(chunk, offset, size, reply)
            }
            Self::Info(info) => {
                let info = info.make_selector_string();
                let info = info.as_bytes();
                read_into(info, offset, size, reply)
            }
        }
    }

    fn write(&mut self, offset: i64, data: &[u8], reply: fuser::ReplyWrite) {
        if offset < 0 {
            reply.error(EINVAL);
            return;
        }

        let offset = offset as usize;

        match self {
            Self::Chunk(chunk) => {
                let end = offset + data.len();

                if end >= MAX_CHUNK_LEN {
                    reply.error(EFBIG);
                    return;
                }

                if end > chunk.len() {
                    chunk.resize(end, 0);
                }

                chunk[offset..end].copy_from_slice(data);

                reply.written(data.len() as u32);
            }
            Self::Info(ct) => {
                if offset != 0 {
                    reply.error(EINVAL);
                    return;
                }

                let data_str = match std::str::from_utf8(data) {
                    Ok(data_str) => data_str,
                    Err(_) => {
                        reply.error(EINVAL);
                        return;
                    }
                };

                let ct_new = match CompressionType::parse_selector_string(data_str) {
                    Some(ct_new) => ct_new,
                    None => {
                        reply.error(EINVAL);
                        return;
                    }
                };

                *ct = ct_new;
                reply.written(data.len() as u32);
            }
        }
    }

    #[inline(always)]
    fn kind(&self) -> FileKind {
        match self {
            Self::Chunk(_) => FileKind::Chunk,
            Self::Info(_) => FileKind::CompressionInfo
        }
    }
}

struct Inode {
    ino: u64,
    x: u8,
    z: u8,
    data: InodeData,
    mtime: SystemTime,
    open_handles: HashMap<u64, FileHandle>,
    linked: bool,
    nlookup: u64
}
impl Inode {
    fn new(chunk: &Chunk<'_>, inos: &InoSet, kind: FileKind) -> Self {
        Self {
            ino: inos.get(kind),
            x: chunk.x,
            z: chunk.z,
            data: InodeData::new(kind, chunk),
            mtime: chunk.mtime,
            open_handles: HashMap::new(),
            linked: true,
            nlookup: 0
        }
    }

    fn blank(x: u8, z: u8, inos: &InoSet, kind: FileKind) -> Self {
        Self {
            ino: inos.get(kind),
            x,
            z,
            data: InodeData::blank(kind),
            mtime: SystemTime::now(),
            open_handles: HashMap::new(),
            linked: true,
            nlookup: 0
        }
    }

    fn attr(&self, writable: bool, uid: u32, gid: u32) -> FileAttr {
        let len = self.data.len();
        let perm = if writable { 0o644 } else { 0o444 };

        fattr(self.ino, len as u64, self.mtime, FileType::RegularFile, perm, self.linked as u32, uid, gid)
    }

    fn inc_lookup(&mut self) {
        self.nlookup += 1;
    }

    fn dec_lookup(&mut self, count: u64) -> u64 {
        if self.nlookup < count {
            error!("Lookup count mismatch detected in {}. It may be wise to remount the smithy filesystem.", self.make_fname());
        }

        self.nlookup -= count;
        self.nlookup
    }

    fn can_discard(&self) -> bool {
        !self.linked && self.nlookup == 0 && self.open_handles.is_empty()
    }

    fn make_fname(&self) -> String {
        self.data.kind().make_fname(self.x, self.z)
    }
}

#[derive(Clone, Copy, Debug)]
struct InoSet {
    chunk_ino: u64,
    info_ino: u64
}
impl InoSet {
    fn get(&self, kind: FileKind) -> u64 {
        match kind {
            FileKind::Chunk => self.chunk_ino,
            FileKind::CompressionInfo => self.info_ino,
        }
    }
}
impl IntoIterator for InoSet {
    type Item = u64;
    type IntoIter = <[u64; 2] as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        [self.chunk_ino, self.info_ino].into_iter()
    }
}

struct InoAlloc(u64);
impl InoAlloc {
    fn new() -> Self {
        Self(FUSE_ROOT_ID + 1)
    }

    fn allocate_inos(&mut self) -> InoSet {
        // round up to next even
        self.0 = (self.0 + 1) & (!1);

        let entry = InoSet {
            chunk_ino: self.0,
            info_ino: self.0 + 1
        };

        self.0 += 2;

        entry
    }
}

struct DirHandle {
    entries: Vec<(u64, FileType, String)>
}

#[derive(Clone, Copy, Debug)]
struct DeletionInfo {
    ino: u64,
    x: u8,
    z: u8,
    kind: FileKind
}
impl From<&mut Inode> for DeletionInfo {
    fn from(value: &mut Inode) -> Self {
        Self {
            ino: value.ino,
            x: value.x,
            z: value.z,
            kind: value.data.kind()
        }
    }
}


pub(crate) struct SmithyFS {
    region: RegionFile,
    uid: u32,
    gid: u32,
    writable: bool,
    root_dir_attr: FileAttr,

    links: HashMap<(u8, u8), InoSet>,
    inodes: HashMap<u64, Inode>,

    dir_handles: HashMap<u64, DirHandle>,

    ino_alloc: InoAlloc,
    fh_alloc: FileHandleAlloc,

    pub(crate) notifier: Arc<Mutex<Option<Notifier>>>
}

impl SmithyFS {
    pub(crate) fn new(region: RegionFile, uid: u32, gid: u32, writable: bool) -> Self {
        let mut fs = Self {
            region,
            uid,
            gid,
            writable,
            root_dir_attr: FileAttr {
                uid,
                gid,
                perm: if writable { 0o755 } else { 0o555 },
                ..ROOT_DIR_ATTR
            },

            links: HashMap::new(),
            inodes: HashMap::new(),

            dir_handles: HashMap::new(),

            ino_alloc: InoAlloc::new(),
            fh_alloc: FileHandleAlloc::new(),

            notifier: Arc::default()
        };

        for z in 0..32 {
            for x in 0..32 {
                let chunk = match fs.region.lookup_chunk(x, z) {
                    Some(c) => c,
                    None => continue,
                };

                let inos = fs.ino_alloc.allocate_inos();

                let chunk_ino = Inode::new(&chunk, &inos, FileKind::Chunk);
                let info_ino = Inode::new(&chunk, &inos, FileKind::CompressionInfo);

                fs.links.insert((x, z), inos);
                fs.inodes.insert(inos.chunk_ino, chunk_ino);
                fs.inodes.insert(inos.info_ino, info_ino);
            }
        }

        fs
    }

    #[inline(always)]
    fn get_ino(&self, key: FileKey) -> Option<u64> {
        let inos = self.links.get(&(key.x, key.z))?;
        Some(inos.get(key.kind))
    }

    #[allow(unused)]
    #[inline(always)]
    fn get_inode(&self, key: FileKey) -> Option<&Inode> {
        let ino = self.get_ino(key)?;
        self.inodes.get(&ino)
    }

    #[inline(always)]
    fn get_inode_mut(&mut self, key: FileKey) -> Option<&mut Inode> {
        let ino = self.get_ino(key)?;
        self.inodes.get_mut(&ino)
    }

    fn stat_ino(&self, ino: u64) -> Option<FileAttr> {
        let inode = self.inodes.get(&ino)?;
        Some(self.stat_inode(inode))
    }

    fn stat_inode(&self, inode: &Inode) -> FileAttr {
        inode.attr(self.writable, self.uid, self.gid)
    }

    fn create_dir_handle(&mut self) -> u64 {
        let fh = self.fh_alloc.alloc();

        let mut entries = vec![
            (FUSE_ROOT_ID, FileType::Directory, ".".to_owned()),
            (FUSE_ROOT_ID, FileType::Directory, "..".to_owned()),
        ];

        entries.reserve_exact(self.inodes.len());

        let kinds = vec![
            FileKind::Chunk,
            FileKind::CompressionInfo
        ];

        for z in 0..32 {
            for x in 0..32 {
                for &kind in &kinds {
                    if let Some(inos) = self.links.get(&(x, z)) {
                        let ino = inos.get(kind);
                        entries.push((ino, FileType::RegularFile, kind.make_fname(x, z)));
                    }
                }
            }
        }

        self.dir_handles.insert(fh, DirHandle { entries });

        fh
    }

    fn gc(&mut self, ino: u64) -> Option<Inode> {
        let inode = self.inodes.get(&ino)?;

        if !inode.can_discard() {
            return None;
        }

        info!("Discarding inode {}", ino);
        self.inodes.remove(&ino)
    }

    fn delete(&mut self, info: DeletionInfo) {
        let ino = info.ino;
        let name = info.kind.make_fname(info.x, info.z);

        if let Ok(guard) = self.notifier.try_lock() {
            guard.as_ref().inspect(|&notifier| {
                let name: std::ffi::OsString = name.into();

                info!("Notifying deletion of inode {}", ino);

                match notifier.inval_entry(FUSE_ROOT_ID, &name) {
                    Ok(_) => info!("Notified deletion of inode {}", ino),
                    Err(e) => warn!("Failed to notify deletion of inode {}: {}", ino, e)
                };
            });
        } else {
            warn!("Failed to acquire notifier lock. Deletion of inode {} will be silent.", ino);
        }
    }
}

impl Filesystem for SmithyFS {
    fn lookup(&mut self, _req: &fuser::Request<'_>, parent: u64, name: &std::ffi::OsStr, reply: fuser::ReplyEntry) {
        if parent != FUSE_ROOT_ID {
            reply.error(ENOENT);
            return;
        }

        if let Some(key) = name.to_str().and_then(FileKey::parse) {
            //debug!("Parsed file name as chunk [{} {}] {:?}", key.x, key.z, key.kind);
            let (writable, uid, gid) = (self.writable, self.uid, self.gid);

            if let Some(inode) = self.get_inode_mut(key) {
                inode.inc_lookup();
                let attr = inode.attr(writable, uid, gid);
                reply.entry(&TTL, &attr, 0);
                return;
            }
            //debug!("Chunk [{} {}] is missing", key.x, key.z);
        }

        //debug!("Failed to look up file {:?} in {}", name, parent);
        reply.error(ENOENT);
    }

    fn forget(&mut self, _req: &fuser::Request<'_>, ino: u64, nlookup: u64) {
        let inode = match self.inodes.get_mut(&ino) {
            Some(inode) => inode,
            None => return
        };

        if inode.dec_lookup(nlookup) == 0 {
            self.gc(ino);
        }
    }

    fn getattr(&mut self, _req: &fuser::Request<'_>, ino: u64, _fh: Option<u64>, reply: fuser::ReplyAttr) {
        if ino == FUSE_ROOT_ID {
            reply.attr(&TTL, &self.root_dir_attr);
        } else if let Some(attr) = self.stat_ino(ino) {
            reply.attr(&TTL, &attr);
        } else {
            reply.error(ENOENT);
        }
    }

    fn mknod(
            &mut self,
            _req: &fuser::Request<'_>,
            parent: u64,
            name: &std::ffi::OsStr,
            mode: u32,
            _umask: u32,
            _rdev: u32,
            reply: fuser::ReplyEntry,
        ) {
        if !self.writable {
            reply.error(EROFS);
            return;
        }

        if parent != FUSE_ROOT_ID {
            reply.error(ENOENT);
            return;
        }

        let file_type = mode & libc::S_IFMT;

        if file_type != libc::S_IFREG {
            reply.error(EPERM);
            return;
        }

        let Some(key) = name.to_str().and_then(FileKey::parse) else {
            reply.error(EINVAL);
            return;
        };

        if self.links.contains_key(&(key.x, key.z)) {
            reply.error(EEXIST);
            return;
        }

        let inos = self.ino_alloc.allocate_inos();
        let chunk_inode = Inode::blank(key.x, key.z, &inos, FileKind::Chunk);
        let info_inode = Inode::blank(key.x, key.z, &inos, FileKind::CompressionInfo);

        warn!("Make sure to set correct compression type in {}", info_inode.make_fname());

        self.links.insert((key.x, key.z), inos);
        self.inodes.insert(inos.chunk_ino, chunk_inode);
        self.inodes.insert(inos.info_ino, info_inode);

        reply.entry(&TTL, &self.stat_ino(inos.get(key.kind)).expect("just-created inode should exist"), 0);
    }

    fn open(&mut self, _req: &fuser::Request<'_>, ino: u64, flags: i32, reply: fuser::ReplyOpen) {
        let (read, write) = match flags & libc::O_ACCMODE {
            libc::O_RDONLY => {
                if flags & libc::O_TRUNC != 0{
                    reply.error(EACCES);
                    return;
                }
                (true, false)
            }
            libc::O_WRONLY => {
                (false, true)
            }
            libc::O_RDWR => {
                (true, true)
            }
            _ => {
                reply.error(EINVAL);
                return;
            }
        };

        if write && !self.writable {
            reply.error(EROFS);
            return;
        }

        let inode = match self.inodes.get_mut(&ino) {
            Some(i) => i,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let fh = self.fh_alloc.alloc();
        inode.open_handles.insert(fh, FileHandle::new(read, write));

        let open_flags = 0;
        reply.opened(fh, open_flags);
    }

    fn opendir(&mut self, _req: &fuser::Request<'_>, ino: u64, _flags: i32, reply: fuser::ReplyOpen) {
        if ino != FUSE_ROOT_ID {
            reply.error(ENOTDIR);
            return;
        }

        let fh = self.create_dir_handle();
        let open_flags = 0;
        reply.opened(fh, open_flags);
    }

    fn read(
            &mut self,
            _req: &fuser::Request<'_>,
            ino: u64,
            fh: u64,
            offset: i64,
            size: u32,
            _flags: i32,
            _lock_owner: Option<u64>,
            reply: fuser::ReplyData,
        ) {
        let inode = match self.inodes.get(&ino) {
            Some(inode) => inode,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let handle = match inode.open_handles.get(&fh) {
            Some(handle) => handle,
            None => {
                reply.error(EBADF);
                return;
            }
        };

        if handle.can_read() {
            inode.data.read(offset, size, reply);
        } else {
            reply.error(EACCES);
        }
    }

    fn write(
            &mut self,
            _req: &fuser::Request<'_>,
            ino: u64,
            fh: u64,
            offset: i64,
            data: &[u8],
            _write_flags: u32,
            _flags: i32,
            _lock_owner: Option<u64>,
            reply: fuser::ReplyWrite,
        ) {
        if !self.writable {
            reply.error(EROFS);
            return;
        }

        let inode = match self.inodes.get_mut(&ino) {
            Some(inode) => inode,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        let handle = match inode.open_handles.get(&fh) {
            Some(handle) => handle,
            None => {
                reply.error(EBADF);
                return;
            }
        };

        if handle.can_write() {
            inode.data.write(offset, data, reply);
        } else {
            reply.error(EACCES);
        }
    }

    fn readdir(
            &mut self,
            _req: &fuser::Request<'_>,
            ino: u64,
            fh: u64,
            offset: i64,
            mut reply: fuser::ReplyDirectory,
        ) {
        if ino != FUSE_ROOT_ID {
            reply.error(ENOENT);
            return;
        }

        let handle = match self.dir_handles.get(&fh) {
            Some(handle) => handle,
            None => {
                reply.error(EBADF);
                return;
            }
        };

        let entries = handle.entries.iter()
            .enumerate()
            .skip(offset as usize);

        for (i, (inode, file_type, name)) in entries {
            if reply.add(*inode, (i + 1) as i64, *file_type, name) {
                break;
            }
        }

        reply.ok();
    }

    fn releasedir(
            &mut self,
            _req: &fuser::Request<'_>,
            _ino: u64,
            fh: u64,
            _flags: i32,
            reply: fuser::ReplyEmpty,
        ) {
        match self.dir_handles.remove(&fh) {
            Some(handle) => {
                drop(handle);
                reply.ok();
            },
            None => reply.error(EBADF),
        }
    }

    fn release(
            &mut self,
            _req: &fuser::Request<'_>,
            ino: u64,
            fh: u64,
            _flags: i32,
            _lock_owner: Option<u64>,
            _flush: bool,
            reply: fuser::ReplyEmpty,
        ) {
        let inode = match self.inodes.get_mut(&ino) {
            Some(inode) => inode,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        match inode.open_handles.remove(&fh) {
            Some(_) => {
                self.gc(ino);

                reply.ok();
                return;
            }
            None => {
                reply.error(EBADF);
                return;
            }
        }
    }

    fn setattr(
            &mut self,
            _req: &fuser::Request<'_>,
            ino: u64,
            mode: Option<u32>,
            uid: Option<u32>,
            gid: Option<u32>,
            size: Option<u64>,
            _atime: Option<fuser::TimeOrNow>,
            _mtime: Option<fuser::TimeOrNow>,
            _ctime: Option<SystemTime>,
            fh: Option<u64>,
            _crtime: Option<SystemTime>,
            _chgtime: Option<SystemTime>,
            _bkuptime: Option<SystemTime>,
            flags: Option<u32>,
            reply: fuser::ReplyAttr,
        ) {
        let inode = match self.inodes.get_mut(&ino) {
            Some(inode) => inode,
            None => {
                reply.error(ENOENT);
                return;
            }
        };

        // truncate
        if let Some(target) = size {
            if !self.writable {
                reply.error(EROFS);
                return;
            }

            if let Some(handle) = fh.and_then(|fh| inode.open_handles.get(&fh)) {
                if !handle.can_write() {
                    reply.error(EACCES);
                    return;
                }
            }

            let target = target as usize;

            match &mut inode.data {
                InodeData::Chunk(chunk) => {
                    if target >= MAX_CHUNK_LEN {
                        reply.error(EFBIG);
                        return;
                    }

                    chunk.resize(target, 0);
                    debug!("Resized ino {:#x?} to {} bytes", ino, target);
                },
                InodeData::Info(_) => {}
            }

            reply.attr(&TTL, &inode.attr(self.writable, self.uid, self.gid));
            return;
        }

        debug!(
            "[Not Implemented] setattr(ino: {:#x?}, mode: {:?}, uid: {:?}, \
            gid: {:?}, size: {:?}, fh: {:?}, flags: {:?})",
            ino, mode, uid, gid, size, fh, flags
        );
        reply.error(ENOSYS);
    }

    fn unlink(&mut self, _req: &fuser::Request<'_>, parent: u64, name: &std::ffi::OsStr, reply: fuser::ReplyEmpty) {
        if parent != FUSE_ROOT_ID {
            reply.error(ENOENT);
            return;
        }

        if let Some(key) = name.to_str().and_then(FileKey::parse) {
            if !key.kind.is_chunk() {
                reply.error(EACCES);
                return;
            }

            let inos = match self.links.remove(&(key.x, key.z)) {
                Some(inos) => inos,
                None => {
                    reply.error(ENOENT);
                    return;
                }
            };

            let mut to_delete = vec![];

            for ino in inos {
                let inode = match self.inodes.get_mut(&ino) {
                    Some(inode) => inode,
                    None => continue
                };

                inode.linked = false;
                to_delete.push(DeletionInfo::from(inode));

                self.gc(ino);
            }

            if !to_delete.is_empty() {
                reply.ok();

                for del_info in to_delete {
                    self.delete(del_info);
                }

                return;
            }
        }

        reply.error(ENOENT);
    }
}
