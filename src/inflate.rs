//! DEFLATE / gzip decompression from scratch.
//!
//! npm and jsr both serve `.tgz` tarballs, so inflate is unavoidable. This
//! is the classic canonical-Huffman decoder (the same shape as zlib's
//! `puff.c`): stored, fixed and dynamic blocks are all supported.

use crate::hash;

/// Decode a gzip member, verifying its CRC-32 trailer.
pub fn gunzip(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 18 || data[0] != 0x1F || data[1] != 0x8B {
        return Err("not a gzip stream".into());
    }
    if data[2] != 8 {
        return Err(format!("unsupported gzip compression method {}", data[2]));
    }
    let flags = data[3];
    let mut i = 10;
    if flags & 0x04 != 0 {
        // FEXTRA
        if i + 2 > data.len() {
            return Err("truncated gzip header".into());
        }
        let xlen = u16::from_le_bytes([data[i], data[i + 1]]) as usize;
        i += 2 + xlen;
    }
    if flags & 0x08 != 0 {
        // FNAME
        while i < data.len() && data[i] != 0 {
            i += 1;
        }
        i += 1;
    }
    if flags & 0x10 != 0 {
        // FCOMMENT
        while i < data.len() && data[i] != 0 {
            i += 1;
        }
        i += 1;
    }
    if flags & 0x02 != 0 {
        // FHCRC
        i += 2;
    }
    if i >= data.len() {
        return Err("truncated gzip header".into());
    }

    let body = &data[i..];
    let mut br = BitReader::new(body);
    let out = inflate_stream(&mut br)?;

    // Trailer: CRC-32 then ISIZE, both little endian.
    let pos = br.byte_pos();
    let rest = &body[pos..];
    if rest.len() >= 8 {
        let want_crc = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
        let got_crc = hash::crc32(&out);
        if want_crc != got_crc {
            return Err(format!(
                "gzip crc mismatch (expected {want_crc:08x}, got {got_crc:08x})"
            ));
        }
    }
    Ok(out)
}

/// Decode a raw DEFLATE stream (as stored inside a zip entry).
pub fn inflate_raw(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut br = BitReader::new(data);
    inflate_stream(&mut br)
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit_buf: u32,
    bit_count: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> BitReader<'a> {
        BitReader {
            data,
            pos: 0,
            bit_buf: 0,
            bit_count: 0,
        }
    }

    fn byte_pos(&self) -> usize {
        // Bits still in the buffer are unconsumed bytes, rounded down.
        self.pos - (self.bit_count / 8) as usize
    }

    fn refill(&mut self) {
        while self.bit_count <= 24 && self.pos < self.data.len() {
            self.bit_buf |= (self.data[self.pos] as u32) << self.bit_count;
            self.pos += 1;
            self.bit_count += 8;
        }
    }

    fn bits(&mut self, n: u32) -> Result<u32, String> {
        if n == 0 {
            return Ok(0);
        }
        if self.bit_count < n {
            self.refill();
            if self.bit_count < n {
                return Err("unexpected end of deflate stream".into());
            }
        }
        let val = self.bit_buf & ((1u32 << n) - 1);
        self.bit_buf >>= n;
        self.bit_count -= n;
        Ok(val)
    }

    fn bit(&mut self) -> Result<u32, String> {
        self.bits(1)
    }

    fn align_to_byte(&mut self) {
        let drop = self.bit_count % 8;
        self.bit_buf >>= drop;
        self.bit_count -= drop;
    }
}

struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn from_lengths(lengths: &[u8]) -> Result<Huffman, String> {
        let mut counts = [0u16; 16];
        for &len in lengths {
            if len > 15 {
                return Err("invalid huffman code length".into());
            }
            counts[len as usize] += 1;
        }
        counts[0] = 0;

        let mut offsets = [0u16; 16];
        for i in 1..15 {
            offsets[i + 1] = offsets[i] + counts[i];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (sym, &len) in lengths.iter().enumerate() {
            if len != 0 {
                let idx = offsets[len as usize] as usize;
                if idx >= symbols.len() {
                    return Err("invalid huffman code lengths".into());
                }
                symbols[idx] = sym as u16;
                offsets[len as usize] += 1;
            }
        }
        Ok(Huffman { counts, symbols })
    }

    fn decode(&self, br: &mut BitReader) -> Result<u16, String> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: usize = 0;
        for len in 1..=15usize {
            code |= br.bit()? as i32;
            let count = self.counts[len] as i32;
            if code - count < first {
                let idx = index + (code - first) as usize;
                return self
                    .symbols
                    .get(idx)
                    .copied()
                    .ok_or_else(|| "invalid huffman symbol".to_string());
            }
            index += count as usize;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err("invalid huffman code".into())
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

fn inflate_stream(br: &mut BitReader) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = br.bit()? == 1;
        let block_type = br.bits(2)?;
        match block_type {
            0 => stored_block(br, &mut out)?,
            1 => {
                let (lit, dist) = fixed_tables();
                decode_block(br, &mut out, &lit, &dist)?;
            }
            2 => {
                let (lit, dist) = dynamic_tables(br)?;
                decode_block(br, &mut out, &lit, &dist)?;
            }
            _ => return Err("invalid deflate block type".into()),
        }
        if last {
            break;
        }
    }
    Ok(out)
}

fn stored_block(br: &mut BitReader, out: &mut Vec<u8>) -> Result<(), String> {
    br.align_to_byte();
    let len = br.bits(16)? as usize;
    let nlen = br.bits(16)? as usize;
    if len != (!nlen & 0xFFFF) {
        return Err("corrupt stored block length".into());
    }
    for _ in 0..len {
        out.push(br.bits(8)? as u8);
    }
    Ok(())
}

fn fixed_tables() -> (Huffman, Huffman) {
    let mut lit_lengths = vec![0u8; 288];
    for (i, slot) in lit_lengths.iter_mut().enumerate() {
        *slot = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let dist_lengths = vec![5u8; 30];
    (
        Huffman::from_lengths(&lit_lengths).expect("fixed literal table is valid"),
        Huffman::from_lengths(&dist_lengths).expect("fixed distance table is valid"),
    )
}

fn dynamic_tables(br: &mut BitReader) -> Result<(Huffman, Huffman), String> {
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let hlit = br.bits(5)? as usize + 257;
    let hdist = br.bits(5)? as usize + 1;
    let hclen = br.bits(4)? as usize + 4;

    let mut cl_lengths = vec![0u8; 19];
    for &slot in ORDER.iter().take(hclen) {
        cl_lengths[slot] = br.bits(3)? as u8;
    }
    let cl_table = Huffman::from_lengths(&cl_lengths)?;

    let mut lengths = vec![0u8; hlit + hdist];
    let mut i = 0;
    while i < lengths.len() {
        let sym = cl_table.decode(br)?;
        match sym {
            0..=15 => {
                lengths[i] = sym as u8;
                i += 1;
            }
            16 => {
                if i == 0 {
                    return Err("repeat code with no previous length".into());
                }
                let prev = lengths[i - 1];
                let repeat = 3 + br.bits(2)? as usize;
                for _ in 0..repeat {
                    if i >= lengths.len() {
                        return Err("code length repeat overflows".into());
                    }
                    lengths[i] = prev;
                    i += 1;
                }
            }
            17 => {
                let repeat = 3 + br.bits(3)? as usize;
                i += repeat;
            }
            18 => {
                let repeat = 11 + br.bits(7)? as usize;
                i += repeat;
            }
            _ => return Err("invalid code length symbol".into()),
        }
    }
    if i > lengths.len() {
        return Err("code length repeat overflows".into());
    }

    let lit = Huffman::from_lengths(&lengths[..hlit])?;
    let dist = Huffman::from_lengths(&lengths[hlit..])?;
    Ok((lit, dist))
}

fn decode_block(
    br: &mut BitReader,
    out: &mut Vec<u8>,
    lit: &Huffman,
    dist: &Huffman,
) -> Result<(), String> {
    loop {
        let sym = lit.decode(br)?;
        match sym {
            0..=255 => out.push(sym as u8),
            256 => return Ok(()),
            257..=285 => {
                let idx = (sym - 257) as usize;
                let length = LENGTH_BASE[idx] as usize + br.bits(LENGTH_EXTRA[idx] as u32)? as usize;
                let dsym = dist.decode(br)? as usize;
                if dsym >= 30 {
                    return Err("invalid distance symbol".into());
                }
                let distance = DIST_BASE[dsym] as usize + br.bits(DIST_EXTRA[dsym] as u32)? as usize;
                if distance == 0 || distance > out.len() {
                    return Err("invalid back-reference distance".into());
                }
                let start = out.len() - distance;
                for k in 0..length {
                    let byte = out[start + k];
                    out.push(byte);
                }
            }
            _ => return Err("invalid literal/length symbol".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inflates_stored_block() {
        // BFINAL=1, BTYPE=00, LEN=3, NLEN=~3, "abc"
        let data = [0x01, 0x03, 0x00, 0xFC, 0xFF, b'a', b'b', b'c'];
        assert_eq!(inflate_raw(&data).unwrap(), b"abc");
    }

    #[test]
    fn gunzip_rejects_non_gzip() {
        assert!(gunzip(b"hello world, not gzip").is_err());
    }
}
