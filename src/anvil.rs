use std::time::{Duration, SystemTime};

#[inline(always)]
fn chunk_meta_addr(chunk_x: u8, chunk_z: u8) -> usize {
        let chunk_x = (chunk_x & 31) as usize;
        let chunk_z = (chunk_z & 31) as usize;

        4 * (chunk_x + chunk_z * 32)
}

#[inline(always)]
fn read_big_endian(raw: &[u8], offset: usize) -> u32 {
    return
          ((raw[0 + offset] as u32) << 24)
        | ((raw[1 + offset] as u32) << 16)
        | ((raw[2 + offset] as u32) << 8)
        | ( raw[3 + offset] as u32);
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RegionFile<'a> {
    data: &'a [u8]
}

impl RegionFile<'_> {
    pub(crate) fn new(data: &[u8]) -> RegionFile {
        RegionFile { data }
    }

    pub(crate) fn lookup_ptr(&self, chunk_x: u8, chunk_z: u8) -> ChunkPtr {
        let base = chunk_meta_addr(chunk_x, chunk_z);

        let o_h = self.data[base + 0] as u32;
        let o_m = self.data[base + 1] as u32;
        let o_l = self.data[base + 2] as u32;
        let len = self.data[base + 3] as u32;

        let offset = (o_h << 16) | (o_m << 8) | o_l;

        ChunkPtr { offset, len }
    }

    pub(crate) fn lookup_timestamp(&self, chunk_x: u8, chunk_z: u8) -> SystemTime {
        let base = chunk_meta_addr(chunk_x, chunk_z);

        let timestamp = read_big_endian(&self.data, base + 0x1000);

        SystemTime::UNIX_EPOCH + Duration::from_secs(timestamp as u64)
    }

    pub(crate) fn lookup_chunk(&self, chunk_x: u8, chunk_z: u8) -> Option<Chunk<'_>> {
        let ptr = self.lookup_ptr(chunk_x, chunk_z);

        if ptr.offset < 2 || ptr.len == 0 {
            return None;
        }

        let start = (ptr.offset as usize) * 4096;
        let len = (ptr.len as usize) * 4096;

        if start + len > self.data.len() {
            return None;
        }

        let chunk_data = &self.data[start..start+len];

        let true_len = read_big_endian(&chunk_data, 0) as usize;
        let compression_type = CompressionType::decode(chunk_data[4]);

        if let CompressionType::Unknown(id) = compression_type {
            if id >= 128 { // stored externally
                return None;
            }
        }

        if true_len <= 1 {
            return None;
        }

        let start = 5;
        let len = true_len - 1;

        if start + len > chunk_data.len() {
            return None;
        }

        let chunk_data = &chunk_data[start..start + len];

        Some(Chunk {
            x: chunk_x & 31,
            z: chunk_z & 31,
            compression_type,
            data: chunk_data
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ChunkPtr {
    /// In sectors, multiply by 4096 to find address
    offset: u32,
    /// In sectors, multiply by 4096 to find true length
    len: u32,
}

#[derive(Clone, Debug)]
pub(crate) enum CompressionType {
    GZip,
    Zlib,
    None,
    LZ4,
    Zstd,
    Unknown(u8),
}
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
        match self {
            &Self::GZip => "[gzip] zlib none lz4 zstd unknown(#)".to_owned(),
            &Self::Zlib => "gzip [zlib] none lz4 zstd unknown(#)".to_owned(),
            &Self::None => "gzip zlib [none] lz4 zstd unknown(#)".to_owned(),
            &Self::LZ4 => "gzip zlib none [lz4] zstd unknown(#)".to_owned(),
            &Self::Zstd => "gzip zlib none lz4 [zstd] unknown(#)".to_owned(),
            &Self::Unknown(id) => format!("gzip zlib none lz4 zstd [unknown({})]", id)
        }
    }

    pub(crate) fn parse_selector_string(selector: &str) -> Option<Self> {
        match &selector.to_ascii_lowercase() as &str {
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
    pub(crate) compression_type: CompressionType,
    pub(crate) data: &'a [u8]
}
