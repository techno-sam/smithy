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

use bitvec::prelude::*;
use log::{debug, info, warn};
use std::{fs::File, io::{Seek, SeekFrom, Write}, time::{Duration, SystemTime}};

pub(crate) const SECTOR_LEN: usize = 0x1000;
const HEADER_SECTORS: usize = 2;
const HEADER_LEN: usize = HEADER_SECTORS * SECTOR_LEN;
pub(crate) const MAX_CHUNK_LEN: usize = SECTOR_LEN * 254;
const MAX_SECTORS: usize = 2_usize.pow(24) - 1 - HEADER_SECTORS;

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
    occupied_sectors: BitVec,
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
                occupied_sectors[(offset as usize - HEADER_SECTORS)..(offset as usize + len as usize - HEADER_SECTORS)].fill(true);
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

    #[inline(always)]
    fn lookup_header_mut(&mut self, chunk_x: u8, chunk_z: u8) -> &mut ChunkHeader {
        let idx = coords_to_idx(chunk_x, chunk_z) as usize;
        &mut self.headers[idx]
    }

    pub(crate) fn lookup_chunk(&self, chunk_x: u8, chunk_z: u8) -> Option<Chunk<'_>> {
        let header = self.lookup_header(chunk_x, chunk_z);
        let addr = header.address?;

        let start = (addr.offset as usize - HEADER_SECTORS) * SECTOR_LEN;
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

    pub(crate) fn delete_chunk(&mut self, chunk_x: u8, chunk_z: u8) {
        let header = self.lookup_header_mut(chunk_x, chunk_z);
        header.set_mtime(SystemTime::now());

        self.free_chunk(chunk_x, chunk_z);
    }

    pub(crate) fn free_chunk(&mut self, chunk_x: u8, chunk_z: u8) {
        let header = self.lookup_header_mut(chunk_x, chunk_z);
        match header.address.take() {
            Some(addr) => {
                let start = addr.offset as usize - HEADER_SECTORS;
                let end = (addr.offset + addr.len) as usize - HEADER_SECTORS;
                let end = end.min(self.occupied_sectors.len());

                if start < end {
                    self.occupied_sectors[start..end].fill(false);
                }
            }
            None => {}
        };
    }

    fn allocate_run(&mut self, len: usize) -> Option<ChunkAddress> {
        // first, try to find a sufficient-length run
        let mut start = 0;

        loop {
            match self.occupied_sectors[start..].first_zero() {
                Some(zero_offset) => {
                    start = start + zero_offset;

                    let search_end = (start + len).min(self.occupied_sectors.len());

                    match self.occupied_sectors[start..search_end].first_one() {
                        Some(one_offset) => { // doesn't fit, try again
                            start = start + one_offset;
                        }
                        None => {
                            let end = start + len;

                            if end >= MAX_SECTORS {
                                return None;
                            }

                            if end > self.occupied_sectors.len() {
                                self.occupied_sectors[start..].fill(true);
                                self.occupied_sectors.resize(end, true);
                            } else {
                                self.occupied_sectors[start..end].fill(true);
                            }

                            return Some(ChunkAddress { offset: (start + HEADER_SECTORS) as u32, len: len as u32 });
                        }
                    }
                }
                None => { // there's no more empty space, allocate
                    start = self.occupied_sectors.len();

                    if start + len >= MAX_SECTORS {
                        return None;
                    }

                    self.occupied_sectors.resize(start + len, true);

                    return Some(ChunkAddress { offset: (start + HEADER_SECTORS) as u32, len: len as u32 });
                }
            }
        }
    }

    pub(crate) fn write_chunk(&mut self, chunk_x: u8, chunk_z: u8, data: &[u8], compression_type: CompressionType, mtime: SystemTime) {
        self.free_chunk(chunk_x, chunk_z);

        if data.len() >= MAX_CHUNK_LEN {
            warn!("Chunk [{} {}] is too long, will silently be deleted", chunk_x, chunk_z);
            return;
        }

        // add 5 bytes for Big Endian u32 length field and u8 compression type field
        let meta_len = ChunkInternalMeta::LEN;
        let container_len = data.len() + meta_len;

        // allocate sectors
        let addr = match self.allocate_run(container_len.div_ceil(SECTOR_LEN)) {
            Some(addr) => addr,
            None => {
                warn!("Failed to allocate sectors for chunk [{} {}], will silently be deleted", chunk_x, chunk_z);
                return;
            }
        };

        // write data
        {
            let start = (addr.offset as usize - HEADER_SECTORS) * SECTOR_LEN;
            let len = (addr.len as usize) * SECTOR_LEN;
            let end = start + len;

            if end > self.chunk_data.len() {
                self.chunk_data.resize(end, 0);
            }

            let container_end = start + container_len;
            if container_end < end {
                self.chunk_data[container_end..end].fill(0);
            }

            // we have to add one to the data len, to account for the compression type field
            let meta = ChunkInternalMeta { length: data.len() + 1, compression_type };
            meta.write(&mut self.chunk_data[start..start+meta_len]);
            self.chunk_data[start+meta_len..container_end].copy_from_slice(data);
        }

        // mark dirty
        {
            let start = addr.offset as usize - HEADER_SECTORS;
            let end = start + (addr.len as usize);

            if end > self.dirty_sectors.len() {
                self.dirty_sectors[start..].fill(true);
                self.dirty_sectors.resize(end, true);
            } else {
                self.dirty_sectors[start..end].fill(true);
            }
        }

        // update header
        let header = self.lookup_header_mut(chunk_x, chunk_z);
        header.set_mtime(mtime);
        header.address = Some(addr);
    }

    pub(crate) fn write_out(&mut self, full_write: bool, file: &mut File) -> std::io::Result<()> {
        // start by truncating/allocating
        let sector_count = self.headers.iter()
            .map(|h| h.address)
            .filter_map(|a| a)
            .map(|a| (a.offset as usize) + (a.len as usize) - HEADER_SECTORS)
            .max()
            .unwrap_or(0);
        file.set_len((HEADER_LEN + sector_count * SECTOR_LEN) as u64)?;

        // always write header
        file.seek(SeekFrom::Start(0))?;

        // write first part of header (locations)
        for idx in 0..(32*32) {
            let header = self.headers[idx];

            let (start, len) = match header.address {
                Some(addr) => (addr.offset, addr.len),
                None => (0, 0),
            };

            let data = [
                ((start >> 16) & 0xff) as u8,
                ((start >>  8) & 0xff) as u8,
                ((start >>  0) & 0xff) as u8,
                len as u8
            ];

            file.write_all(&data)?
        }

        // write second part of header (timestamps)
        for idx in 0..(32*32) {
            let header = self.headers[idx];
            let mtime = header.mtime;

            let data = [
                ((mtime >> 24) & 0xff) as u8,
                ((mtime >> 16) & 0xff) as u8,
                ((mtime >>  8) & 0xff) as u8,
                ((mtime >>  0) & 0xff) as u8,
            ];

            file.write_all(&data)?;
        }

        // write (changed) sectors

        let sector_idx_iter: Box<dyn Iterator<Item=usize>> = if full_write {
            Box::new((0..sector_count).into_iter())
        } else {
            Box::new(self.dirty_sectors.iter_ones().take_while(|idx| *idx < sector_count))
        };

        for sector_idx in sector_idx_iter {
            if !full_write {
                info!("> Writing sector {:#06x}", sector_idx);
            }

            let start = sector_idx * SECTOR_LEN;
            let end = start + SECTOR_LEN;

            file.seek(SeekFrom::Start((HEADER_LEN + start) as u64))?;
            file.write_all(&self.chunk_data[start..end])?;
        }

        file.set_modified(SystemTime::now())?;
        file.flush()?;
        file.sync_all()?;

        self.dirty_sectors.fill(false);

        Ok(())
    }
}

struct ChunkInternalMeta {
    /// Note: includes the byte used to describe compression_type
    length: usize,
    compression_type: CompressionType
}

impl ChunkInternalMeta {
    /// unit: bytes
    const LEN: usize = 5;

    fn read(raw: &[u8]) -> Self {
        let length = read_big_endian(&raw, 0) as usize;
        let compression_type = CompressionType::decode(raw[4]);

        Self { length, compression_type }
    }

    fn write(&self, raw: &mut [u8]) {
        raw[0] = ((self.length >> 24) & 0xff) as u8;
        raw[1] = ((self.length >> 16) & 0xff) as u8;
        raw[2] = ((self.length >>  8) & 0xff) as u8;
        raw[3] = ((self.length >>  0) & 0xff) as u8;
        raw[4] = self.compression_type.encode();
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

    fn set_mtime(&mut self, time: SystemTime) {
        self.mtime = match time.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(dur) => dur.as_secs() as u32,
            Err(_) => 0
        };
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
        let selector = selector.to_ascii_lowercase();
        let selector: &str = selector.trim();

        let out = match selector {
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
        };

        if out.is_some() {
            return out;
        }

        let start = selector.find('[')? + 1;
        let len = selector[start..].find(']')?;

        if len > 0 {
            let part = &selector[start..start+len];
            debug!("Recursively parsing `{}` (from `{}`)", part, selector);
            return Self::parse_selector_string(&selector[start..start+len]);
        }

        None
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
