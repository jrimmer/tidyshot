//! DEFLATE, zlib framing, CRC-32 and Adler-32.
//!
//! The build is offline and no compression crate is vendored, so the project
//! reader carries its own inflate. Writing uses stored blocks: a project is
//! written once and read once, so the small size increase is worth avoiding a
//! compressor in the trusted surface.

use std::fmt;

/// A DEFLATE or zlib stream that could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressError {
    message: String,
}

impl CompressError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CompressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CompressError {}

/// CRC-32 as used by ZIP entries (IEEE polynomial, reflected).
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

/// Adler-32 as used by zlib framing.
#[must_use]
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let mut a = 1u32;
    let mut b = 0u32;
    for byte in data {
        a = (a + u32::from(*byte)) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

/// A least-significant-bit-first bit reader.
///
/// DEFLATE packs data elements starting at the least significant bit of each
/// byte; Huffman codes are then read in the order they are packed, so the
/// first bit read is the most significant bit of the code.
struct BitReader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn bit(&mut self) -> Result<u32, CompressError> {
        let byte = self.position / 8;
        if byte >= self.data.len() {
            return Err(CompressError::new("truncated deflate stream"));
        }
        let shift = self.position % 8;
        self.position += 1;
        Ok(u32::from((self.data[byte] >> shift) & 1))
    }

    /// Read bits starting with the least-significant bit of the value.
    fn bits_le(&mut self, count: u32) -> Result<u32, CompressError> {
        let mut value = 0u32;
        for index in 0..count {
            value |= self.bit()? << index;
        }
        Ok(value)
    }

    fn align(&mut self) {
        self.position = self.position.div_ceil(8) * 8;
    }

    fn byte(&mut self) -> Result<u8, CompressError> {
        self.align();
        let index = self.position / 8;
        if index >= self.data.len() {
            return Err(CompressError::new("truncated deflate stream"));
        }
        self.position += 8;
        Ok(self.data[index])
    }
}

/// A canonical Huffman decoding table keyed by `(length, code)`.
struct Huffman {
    /// Sorted `(code, length, symbol)` entries.
    entries: Vec<(u32, u32, u32)>,
    max_length: u32,
}

impl Huffman {
    fn build(lengths: &[u32]) -> Result<Self, CompressError> {
        let max_length = lengths.iter().copied().max().unwrap_or(0);
        if max_length == 0 {
            return Ok(Self {
                entries: Vec::new(),
                max_length: 0,
            });
        }
        let mut counts = vec![0u32; (max_length + 1) as usize];
        for &length in lengths {
            if length > 0 {
                counts[length as usize] += 1;
            }
        }
        // Kraft inequality check: an over-subscribed code is invalid.
        let mut left = 1i64;
        for length in 1..=max_length {
            left = (left << 1) - i64::from(counts[length as usize]);
            if left < 0 {
                return Err(CompressError::new("over-subscribed huffman code"));
            }
        }
        let mut next_code = vec![0u32; (max_length + 1) as usize];
        let mut code = 0u32;
        for length in 1..=max_length {
            code = (code + counts[length as usize - 1]) << 1;
            next_code[length as usize] = code;
        }
        let mut entries = Vec::new();
        for (symbol, &length) in lengths.iter().enumerate() {
            if length == 0 {
                continue;
            }
            let assigned = next_code[length as usize];
            next_code[length as usize] += 1;
            entries.push((assigned, length, symbol as u32));
        }
        entries.sort_unstable();
        Ok(Self {
            entries,
            max_length,
        })
    }

    fn decode(&self, reader: &mut BitReader<'_>) -> Result<u32, CompressError> {
        let mut code = 0u32;
        for length in 1..=self.max_length {
            code = (code << 1) | reader.bit()?;
            // Linear scan is fine for the small tables DEFLATE uses per block.
            for &(candidate, candidate_length, symbol) in &self.entries {
                if candidate_length == length && candidate == code {
                    return Ok(symbol);
                }
            }
        }
        Err(CompressError::new("invalid huffman code"))
    }
}

const LENGTH_BASE: [u32; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DISTANCE_BASE: [u32; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DISTANCE_EXTRA: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Inflate a raw DEFLATE stream.
pub fn inflate(data: &[u8]) -> Result<Vec<u8>, CompressError> {
    let mut reader = BitReader::new(data);
    let mut out = Vec::new();
    loop {
        let final_block = reader.bits_le(1)? == 1;
        let kind = reader.bits_le(2)?;
        match kind {
            0 => {
                let length = u16::from_le_bytes([reader.byte()?, reader.byte()?]);
                let complement = u16::from_le_bytes([reader.byte()?, reader.byte()?]);
                if length != !complement {
                    return Err(CompressError::new("stored block length mismatch"));
                }
                for _ in 0..length {
                    out.push(reader.byte()?);
                }
            }
            1 => {
                let lengths = fixed_lengths();
                let literal = Huffman::build(&lengths.0)?;
                let distance = Huffman::build(&lengths.1)?;
                inflate_block(&mut reader, &literal, &distance, &mut out)?;
            }
            2 => {
                let (literal_lengths, distance_lengths) = read_dynamic_tables(&mut reader)?;
                let literal = Huffman::build(&literal_lengths)?;
                let distance = Huffman::build(&distance_lengths)?;
                inflate_block(&mut reader, &literal, &distance, &mut out)?;
            }
            _ => return Err(CompressError::new("reserved deflate block type")),
        }
        if final_block {
            return Ok(out);
        }
    }
}

fn fixed_lengths() -> (Vec<u32>, Vec<u32>) {
    let mut literal = vec![0u32; 288];
    for (index, slot) in literal.iter_mut().enumerate() {
        *slot = match index {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    (literal, vec![5u32; 30])
}

fn read_dynamic_tables(reader: &mut BitReader<'_>) -> Result<(Vec<u32>, Vec<u32>), CompressError> {
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let literal_count = reader.bits_le(5)? as usize + 257;
    let distance_count = reader.bits_le(5)? as usize + 1;
    let code_count = reader.bits_le(4)? as usize + 4;
    let mut code_lengths = vec![0u32; 19];
    for index in 0..code_count {
        code_lengths[ORDER[index]] = reader.bits_le(3)?;
    }
    let code_table = Huffman::build(&code_lengths)?;
    let total = literal_count + distance_count;
    let mut lengths = Vec::with_capacity(total);
    while lengths.len() < total {
        let symbol = code_table.decode(reader)?;
        match symbol {
            0..=15 => lengths.push(symbol),
            16 => {
                let previous = *lengths
                    .last()
                    .ok_or_else(|| CompressError::new("invalid repeat"))?;
                let repeat = reader.bits_le(2)? + 3;
                for _ in 0..repeat {
                    lengths.push(previous);
                }
            }
            17 => {
                let repeat = reader.bits_le(3)? + 3;
                lengths.extend(std::iter::repeat_n(0, repeat as usize));
            }
            18 => {
                let repeat = reader.bits_le(7)? + 11;
                lengths.extend(std::iter::repeat_n(0, repeat as usize));
            }
            _ => return Err(CompressError::new("invalid code length symbol")),
        }
    }
    if lengths.len() != total {
        return Err(CompressError::new("code length overflow"));
    }
    let literal = lengths[..literal_count].to_vec();
    let distance = lengths[literal_count..].to_vec();
    Ok((literal, distance))
}

fn inflate_block(
    reader: &mut BitReader<'_>,
    literal: &Huffman,
    distance: &Huffman,
    out: &mut Vec<u8>,
) -> Result<(), CompressError> {
    loop {
        let symbol = literal.decode(reader)?;
        match symbol {
            0..=255 => out.push(symbol as u8),
            256 => return Ok(()),
            257..=285 => {
                let index = (symbol - 257) as usize;
                let length = LENGTH_BASE[index] + reader.bits_le(LENGTH_EXTRA[index])?;
                let dist_symbol = distance.decode(reader)?;
                if dist_symbol as usize >= DISTANCE_BASE.len() {
                    return Err(CompressError::new("invalid distance symbol"));
                }
                let dist_index = dist_symbol as usize;
                let distance =
                    DISTANCE_BASE[dist_index] + reader.bits_le(DISTANCE_EXTRA[dist_index])?;
                if distance == 0 || distance as usize > out.len() {
                    return Err(CompressError::new("distance beyond output"));
                }
                let start = out.len() - distance as usize;
                for offset in 0..length as usize {
                    let byte = out[start + offset];
                    out.push(byte);
                }
            }
            _ => return Err(CompressError::new("invalid literal symbol")),
        }
    }
}

/// Wrap `data` in a zlib stream using stored DEFLATE blocks.
#[must_use]
pub fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 65_535 * 5 + 16);
    // 0x78 0x01: deflate, 32 KiB window, no preset dictionary, fastest level.
    out.push(0x78);
    out.push(0x01);
    let mut offset = 0;
    loop {
        let remaining = data.len() - offset;
        let chunk = remaining.min(65_535);
        let final_block = offset + chunk == data.len();
        out.push(if final_block { 1 } else { 0 });
        let length = chunk as u16;
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(&data[offset..offset + chunk]);
        offset += chunk;
        if final_block {
            break;
        }
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Inflate a zlib-framed stream and verify its Adler-32 trailer.
pub fn zlib_uncompress(data: &[u8]) -> Result<Vec<u8>, CompressError> {
    if data.len() < 6 {
        return Err(CompressError::new("truncated zlib stream"));
    }
    let method = data[0];
    if method & 0x0f != 8 {
        return Err(CompressError::new("unsupported zlib method"));
    }
    let expected = u32::from_be_bytes(
        data[data.len() - 4..]
            .try_into()
            .map_err(|_| CompressError::new("truncated zlib trailer"))?,
    );
    let payload = inflate(&data[2..data.len() - 4])?;
    if adler32(&payload) != expected {
        return Err(CompressError::new("zlib checksum mismatch"));
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_and_adler_match_known_values() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn stored_blocks_round_trip() {
        for size in [0usize, 1, 100, 70_000] {
            let data: Vec<u8> = (0..size).map(|index| (index % 251) as u8).collect();
            let compressed = zlib_store(&data);
            assert_eq!(zlib_uncompress(&compressed).unwrap(), data, "size {size}");
        }
    }

    #[test]
    fn inflate_decodes_fixed_and_dynamic_blocks() {
        // Streams produced by zlib: a fixed-Huffman block and a dynamic block.
        let fixed: &[u8] = &[
            0x78, 0x01, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x57, 0xc8, 0x40, 0x27, 0xb9, 0x00, 0x70,
            0xbe, 0x08, 0xbb,
        ];
        assert_eq!(
            zlib_uncompress(fixed).unwrap(),
            b"hello hello hello hello\n"
        );
        let dynamic: &[u8] = &[
            0x78, 0x01, 0x4b, 0x4c, 0x4a, 0x4e, 0x1c, 0x08, 0x04, 0x00, 0xdb, 0xf6, 0x2d, 0xf1,
        ];
        assert_eq!(zlib_uncompress(dynamic).unwrap(), b"abc".repeat(40));

        // A level-9 zlib stream that uses a dynamic Huffman block.
        let mixed: &[u8] = &[
            0x78, 0xda, 0xed, 0xc9, 0x51, 0x0a, 0x00, 0x10, 0x10, 0x45, 0xd1, 0xad, 0xcc, 0xd6,
            0x9e, 0x4c, 0xa8, 0x19, 0x14, 0x5f, 0x56, 0x8f, 0x6d, 0xf0, 0x7e, 0x6e, 0xb7, 0x0e,
            0xac, 0x67, 0x48, 0xd0, 0x09, 0x49, 0x70, 0x87, 0x44, 0xb5, 0xf3, 0xda, 0x47, 0xb1,
            0x56, 0x05, 0x64, 0x32, 0x99, 0x4c, 0xfe, 0x81, 0xd7, 0x05, 0x86, 0x61, 0x9e, 0xce,
            0x06, 0xff, 0x56, 0xa2, 0x10,
        ];
        let expected: Vec<u8> = format!(
            "{}{}",
            "alpha beta gamma delta epsilon ".repeat(50),
            "zeta ".repeat(200)
        )
        .into_bytes();
        assert_eq!(zlib_uncompress(mixed).unwrap(), expected);
    }

    #[test]
    fn corrupt_streams_are_rejected() {
        assert!(zlib_uncompress(b"x").is_err());
        let mut data = zlib_store(b"payload");
        let last = data.len() - 1;
        data[last] ^= 0xff;
        assert!(zlib_uncompress(&data).is_err());
    }
}
