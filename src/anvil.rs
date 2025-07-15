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

    pub(crate) fn lookup_chunk(&self, chunk_x: u8, chunk_z: u8) -> Chunk<'_> {
        let ptr = self.lookup_ptr(chunk_x, chunk_z);

        let start = (ptr.offset as usize) * 4096;
        let len = (ptr.len as usize) * 4096;

        let chunk_data = &self.data[start..start+len];

        let true_len = read_big_endian(&chunk_data, 0) as usize;
        //let compression_type = chunk_data[4];
        let chunk_data = &chunk_data[5..5 + true_len - 1];

        Chunk {
            x: chunk_x & 31,
            z: chunk_z & 31,
            data: chunk_data
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ChunkPtr {
    /// In sectors, multiply by 4096 to find address
    offset: u32,
    /// In sectors, multiply by 4096 to find true length
    len: u32,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Chunk<'a> {
    pub(crate) x: u8,
    pub(crate) z: u8,
    pub(crate) data: &'a [u8]
}
