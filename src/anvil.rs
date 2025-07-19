use bitvec::prelude::*;
use log::warn;
use std::time::{Duration, SystemTime};

pub(crate) const SECTOR_LEN: usize = 0x1000;
const HEADER_LEN: usize = 0x2000;
pub(crate) const MAX_CHUNK_LEN: usize = SECTOR_LEN * 254;

#[inline(always)]
pub(crate) fn coords_to_idx(x: u8, z: u8) -> usize {
    (x as usize & 31) | ((z as usize & 31) << 5)
}

#[inline(always)]
pub(crate) fn idx_to_coords(idx: usize) -> (u8, u8) {
    ((idx & 31) as u8, ((idx >> 5) & 31) as u8)
}

#[inline(always)]
fn read_big_endian(raw: &[u8], offset: usize) -> u32 {
    return
          ((raw[0 + offset] as u32) << 24)
        | ((raw[1 + offset] as u32) << 16)
        | ((raw[2 + offset] as u32) << 8)
        | ( raw[3 + offset] as u32);
}

#[inline(always)]
fn false_bitvec(len: usize) -> BitVec {
    bitvec![0; len]
}

#[derive(Clone, Debug)]
pub(crate) struct RegionFile {
    headers: Box<[ChunkHeader; 32 * 32]>,
    chunk_data: Vec<u8>,
    #[allow(unused)]
    occupied_sectors: BitVec,
    #[allow(unused)]
    dirty_sectors: BitVec
}

impl RegionFile {
    pub(crate) fn new(data: Vec<u8>) -> Self {
        let (header_data, chunk_data, sector_count) = {
            let mut header_data = data;
            let mut chunk_data = header_data.split_off(HEADER_LEN);

            let sector_count = chunk_data.len().div_ceil(SECTOR_LEN);

            // Pad chunk_data out to a whole-number sector length
            chunk_data.resize(sector_count * SECTOR_LEN, 0);

            (header_data, chunk_data, sector_count)
        };

        let mut headers: Vec<ChunkHeader> = Vec::with_capacity(32 * 32);
        let mut occupied_sectors = false_bitvec(sector_count);
        let dirty_sectors = false_bitvec(sector_count);

        for idx in 0..(32*32) {
            let base = 4 * idx;
            let (x, z) = idx_to_coords(idx);

            // Read raw metadata
            let pos_info = read_big_endian(&header_data, base);
            let offset = (pos_info >> 8) & 0xff_ff_ff;
            let len = pos_info & 0xff;
            let mtime = read_big_endian(&header_data, base + 0x1000);

            // avoid displaying illegal length warning if this fact is already known
            let known_invalid = offset < 2 || len == 0;

            let header = {
                let mut header = ChunkHeader::new(offset, len, mtime, sector_count as u32);

                // Extended validation
                if let Some(addr) = header.address {
                    let byte_offset = (addr.offset as usize - 2) * SECTOR_LEN;
                    let byte_len = (addr.len as usize) * SECTOR_LEN;

                    let chunk_specific_data = &chunk_data[byte_offset..byte_offset+byte_len];
                    let meta = ChunkInternalMeta::read(chunk_specific_data);

                    if match meta.compression_type {
                        // msb is used to mark chunk as stored externally
                        CompressionType::Unknown(id) if id >= 128 => true,
                        _ => false
                    } {
                        panic!("Chunk [{x} {z}] is stored externally to the region file. Smithy cannot handle such cases.");
                    }

                    // add 4 bytes for the length field itself
                    if meta.length <= 1 || meta.length + 4 > chunk_specific_data.len() {
                        header.address = None;
                        warn!("Chunk [{x} {z}] has an illegal length and will be deleted on write");
                    }
                } else if !known_invalid {
                    warn!("Chunk [{x} {z}] has an invalid header and will be deleted on write");
                }

                header
            };

            if header.valid() {
                occupied_sectors[(offset as usize)..(offset as usize + len as usize)].fill(true);
            }

            headers.push(header);
        }

        let headers: Box<[ChunkHeader; 32 * 32]> = headers.try_into().unwrap();

        Self {
            headers,
            chunk_data,
            occupied_sectors,
            dirty_sectors
        }
    }

    #[inline(always)]
    fn lookup_header(&self, chunk_x: u8, chunk_z: u8) -> &ChunkHeader {
        let idx = coords_to_idx(chunk_x, chunk_z) as usize;
        &self.headers[idx]
    }

    #[allow(unused)]
    #[inline(always)]
    pub(crate) fn lookup_timestamp(&self, chunk_x: u8, chunk_z: u8) -> SystemTime {
        self.lookup_header(chunk_x, chunk_z).mtime()
    }

    pub(crate) fn lookup_chunk(&self, chunk_x: u8, chunk_z: u8) -> Option<Chunk<'_>> {
        let header = self.lookup_header(chunk_x, chunk_z);
        let addr = header.address?;

        let start = (addr.offset as usize - 2) * SECTOR_LEN;
        let len = (addr.len as usize) * SECTOR_LEN;

        let chunk_data = &self.chunk_data[start..start+len];

        let meta = ChunkInternalMeta::read(&chunk_data);

        let start = 5;
        let len = meta.length - 1;

        let chunk_data = &chunk_data[start..start + len];

        Some(Chunk {
            x: chunk_x & 31,
            z: chunk_z & 31,
            mtime: header.mtime(),
            compression_type: meta.compression_type,
            data: chunk_data
        })
    }
}

struct ChunkInternalMeta {
    /// Note: includes the byte used to describe compression_type
    length: usize,
    compression_type: CompressionType
}

impl ChunkInternalMeta {
    fn read(raw: &[u8]) -> Self {
        let length = read_big_endian(&raw, 0) as usize;
        let compression_type = CompressionType::decode(raw[4]);

        Self { length, compression_type }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ChunkAddress {
    /// In sectors, must be >= 2
    offset: u32,
    /// In sectors, must be > 0
    len: u32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ChunkHeader {
    /// None if invalid
    address: Option<ChunkAddress>,
    /// Modification time, in epoch seconds
    mtime: u32,
}

impl ChunkHeader {
    fn new(offset: u32, len: u32, mtime: u32, sector_count: u32) -> Self {
        let address = if offset >= 2 && len > 0 && (offset + len - 2) <= sector_count {
            Some(ChunkAddress { offset, len })
        } else {
            None
        };

        Self { address, mtime }
    }

    #[inline(always)]
    pub(crate) fn valid(&self) -> bool {
        self.address.is_some()
    }

    fn mtime(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(self.mtime as u64)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum CompressionType {
    GZip,
    Zlib,
    None,
    LZ4,
    Zstd,
    Unknown(u8),
}
#[allow(unused)]
impl CompressionType {
    fn decode(id: u8) -> Self {
        match id {
             1 => Self::GZip,
             2 => Self::Zlib,
             3 => Self::None,
             4 => Self::LZ4,
            53 => Self::Zstd,

            id => Self::Unknown(id)
        }
    }

    fn encode(&self) -> u8 {
        match self {
            &Self::GZip => 1,
            &Self::Zlib => 2,
            &Self::None => 3,
            &Self::LZ4 => 4,
            &Self::Zstd => 53,

            &Self::Unknown(id) => id
        }
    }

    pub(crate) fn make_selector_string(&self) -> String {
        let out = match self {
            &Self::GZip => "[gzip] zlib none lz4 zstd unknown(#)".to_owned(),
            &Self::Zlib => "gzip [zlib] none lz4 zstd unknown(#)".to_owned(),
            &Self::None => "gzip zlib [none] lz4 zstd unknown(#)".to_owned(),
            &Self::LZ4 => "gzip zlib none [lz4] zstd unknown(#)".to_owned(),
            &Self::Zstd => "gzip zlib none lz4 [zstd] unknown(#)".to_owned(),
            &Self::Unknown(id) => format!("gzip zlib none lz4 zstd [unknown({})]", id)
        };
        out + "\n"
    }

    pub(crate) fn parse_selector_string(selector: &str) -> Option<Self> {
        match &selector.to_ascii_lowercase().trim() as &str {
            "gzip" => Some(Self::GZip),
            "zlib" => Some(Self::Zlib),
            "none" => Some(Self::None),
            "lz4"  => Some(Self::LZ4),
            "zstd" => Some(Self::Zstd),
            mut s  => {
                if s.starts_with("unknown(") && s.ends_with(")") {
                    s = &s[8..s.len()-1];
                }

                s.parse::<u8>()
                    .ok()
                    .map(Self::decode)
            }
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct Chunk<'a> {
    pub(crate) x: u8,
    pub(crate) z: u8,
    pub(crate) mtime: SystemTime,
    pub(crate) compression_type: CompressionType,
    pub(crate) data: &'a [u8]
}
