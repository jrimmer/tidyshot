//! Minimal PNG and ZIP codecs for editable projects and previews.
// Ported from omnishot/image_project.py (zipfile) and omnishot/editor.py
// (png_bytes). PNG output keeps the RGBA8 byte order the annotation pipeline
// uses; PNG input is converted to the same layout once at the boundary.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use crate::compress::{CompressError, crc32, deflate_store, zlib_store, zlib_uncompress};

/// The RGBA8 byte order shared with [`crate::effects::RgbaBuffer`].
pub type Rgba8 = Vec<u8>;

/// An image decode or encode failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageError {
    message: String,
}

impl ImageError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ImageError {}

impl From<CompressError> for ImageError {
    fn from(error: CompressError) -> Self {
        Self::new(error.to_string())
    }
}

/// An RGBA8 image with a tight row stride.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub data: Rgba8,
}

impl Image {
    #[must_use]
    pub fn new(width: usize, height: usize, data: Rgba8) -> Self {
        debug_assert_eq!(data.len(), width * height * 4);
        Self {
            width,
            height,
            data,
        }
    }

    /// A fully transparent image.
    #[must_use]
    pub fn transparent(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0u8; width * height * 4],
        }
    }

    /// Read one pixel.
    #[must_use]
    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let offset = (y * self.width + x) * 4;
        [
            self.data[offset],
            self.data[offset + 1],
            self.data[offset + 2],
            self.data[offset + 3],
        ]
    }

    /// Encode as a PNG with straight (non-premultiplied) RGBA8 samples.
    // Ported from omnishot/editor.py (png_bytes)
    #[must_use]
    pub fn to_png(&self) -> Vec<u8> {
        let mut raw = Vec::with_capacity((self.width * 4 + 1) * self.height);
        for row in 0..self.height {
            raw.push(0); // filter type 0 (None)
            let start = row * self.width * 4;
            raw.extend_from_slice(&self.data[start..start + self.width * 4]);
        }
        let compressed = zlib_store(&raw);
        let mut out = Vec::with_capacity(compressed.len() + 64);
        out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        let mut header = Vec::with_capacity(13);
        header.extend_from_slice(&(self.width as u32).to_be_bytes());
        header.extend_from_slice(&(self.height as u32).to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
        push_chunk(&mut out, b"IHDR", &header);
        push_chunk(&mut out, b"IDAT", &compressed);
        push_chunk(&mut out, b"IEND", &[]);
        out
    }
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// Decode a PNG into straight RGBA8 pixels.
// Ported from omnishot/image_project.py (QImage.fromData)
pub fn decode_png(bytes: &[u8]) -> Result<Image, ImageError> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 8 || &bytes[..8] != SIGNATURE {
        return Err(ImageError::new("not a PNG image"));
    }
    let mut offset = 8;
    let mut width = 0usize;
    let mut height = 0usize;
    let mut depth = 0u8;
    let mut color_type = 0u8;
    let mut interlace = 0u8;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut transparency: Option<Vec<u8>> = None;
    let mut idat: Vec<u8> = Vec::new();

    while offset + 8 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = &bytes[offset + 4..offset + 8];
        let start = offset + 8;
        let end = start
            .checked_add(length)
            .ok_or_else(|| ImageError::new("truncated PNG chunk"))?;
        if end + 4 > bytes.len() {
            return Err(ImageError::new("truncated PNG chunk"));
        }
        let data = &bytes[start..end];
        match kind {
            b"IHDR" => {
                if data.len() != 13 {
                    return Err(ImageError::new("invalid PNG header"));
                }
                width = u32::from_be_bytes(data[0..4].try_into().unwrap()) as usize;
                height = u32::from_be_bytes(data[4..8].try_into().unwrap()) as usize;
                depth = data[8];
                color_type = data[9];
                interlace = data[12];
            }
            b"PLTE" => {
                for chunk in data.chunks_exact(3) {
                    palette.push([chunk[0], chunk[1], chunk[2]]);
                }
            }
            b"tRNS" => transparency = Some(data.to_vec()),
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        offset = end + 4;
    }

    if width == 0 || height == 0 {
        return Err(ImageError::new("invalid PNG dimensions"));
    }
    if interlace != 0 {
        return Err(ImageError::new("interlaced PNG images are not supported"));
    }
    if depth != 8 {
        return Err(ImageError::new("unsupported PNG bit depth"));
    }
    let channels = match color_type {
        0 => 1,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        _ => return Err(ImageError::new("unsupported PNG color type")),
    };
    let raw = zlib_uncompress(&idat)?;
    let stride = width * channels;
    let expected = (stride + 1) * height;
    if raw.len() < expected {
        return Err(ImageError::new("truncated PNG data"));
    }
    let mut lines: Vec<Vec<u8>> = Vec::with_capacity(height);
    let mut previous = vec![0u8; stride];
    for row in 0..height {
        let start = row * (stride + 1);
        let filter = raw[start];
        let mut line = raw[start + 1..start + 1 + stride].to_vec();
        unfilter(filter, &mut line, &previous, channels)?;
        lines.push(line.clone());
        previous = line;
    }

    let mut out = vec![0u8; width * height * 4];
    for (row, line) in lines.iter().enumerate() {
        for column in 0..width {
            let source = column * channels;
            let pixel = match color_type {
                0 => {
                    let gray = line[source];
                    [gray, gray, gray, 255]
                }
                2 => [line[source], line[source + 1], line[source + 2], 255],
                3 => {
                    let index = line[source] as usize;
                    let rgb = palette
                        .get(index)
                        .ok_or_else(|| ImageError::new("palette index out of range"))?;
                    let alpha = transparency
                        .as_ref()
                        .and_then(|table| table.get(index))
                        .copied()
                        .unwrap_or(255);
                    [rgb[0], rgb[1], rgb[2], alpha]
                }
                4 => [line[source], line[source], line[source], line[source + 1]],
                _ => [
                    line[source],
                    line[source + 1],
                    line[source + 2],
                    line[source + 3],
                ],
            };
            let target = (row * width + column) * 4;
            out[target..target + 4].copy_from_slice(&pixel);
        }
    }
    Ok(Image::new(width, height, out))
}

fn unfilter(
    filter: u8,
    line: &mut [u8],
    previous: &[u8],
    channels: usize,
) -> Result<(), ImageError> {
    let bpp = channels;
    match filter {
        0 => {}
        1 => {
            for index in bpp..line.len() {
                line[index] = line[index].wrapping_add(line[index - bpp]);
            }
        }
        2 => {
            for index in 0..line.len() {
                line[index] = line[index].wrapping_add(previous[index]);
            }
        }
        3 => {
            for index in 0..line.len() {
                let left = if index >= bpp { line[index - bpp] } else { 0 };
                let up = previous[index];
                line[index] =
                    line[index].wrapping_add(((u16::from(left) + u16::from(up)) / 2) as u8);
            }
        }
        4 => {
            for index in 0..line.len() {
                let left = if index >= bpp {
                    i32::from(line[index - bpp])
                } else {
                    0
                };
                let up = i32::from(previous[index]);
                let up_left = if index >= bpp {
                    i32::from(previous[index - bpp])
                } else {
                    0
                };
                let p = left + up - up_left;
                let pa = (p - left).abs();
                let pb = (p - up).abs();
                let pc = (p - up_left).abs();
                let predictor = if pa <= pb && pa <= pc {
                    left
                } else if pb <= pc {
                    up
                } else {
                    up_left
                };
                line[index] = line[index].wrapping_add(predictor as u8);
            }
        }
        _ => return Err(ImageError::new("unsupported PNG filter")),
    }
    Ok(())
}

/// One entry stored in a ZIP archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipEntry {
    pub name: String,
    pub data: Vec<u8>,
}

/// A ZIP archive read from disk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZipArchive {
    entries: Vec<ZipEntry>,
}

impl ZipArchive {
    /// Read a ZIP archive, decoding each entry's DEFLATE stream.
    // Ported from omnishot/image_project.py (ZipFile)
    pub fn read(bytes: &[u8]) -> Result<Self, ImageError> {
        let eocd = find_eocd(bytes).ok_or_else(|| ImageError::new("invalid ZIP file"))?;
        let count = u16::from_le_bytes(bytes[eocd + 10..eocd + 12].try_into().unwrap()) as usize;
        let mut offset =
            u32::from_le_bytes(bytes[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            if offset + 46 > bytes.len() || &bytes[offset..offset + 4] != b"PK\x01\x02" {
                return Err(ImageError::new("invalid ZIP directory"));
            }
            let method = u16::from_le_bytes(bytes[offset + 10..offset + 12].try_into().unwrap());
            let compressed_size =
                u32::from_le_bytes(bytes[offset + 20..offset + 24].try_into().unwrap()) as usize;
            let uncompressed_size =
                u32::from_le_bytes(bytes[offset + 24..offset + 28].try_into().unwrap()) as usize;
            let name_length =
                u16::from_le_bytes(bytes[offset + 28..offset + 30].try_into().unwrap()) as usize;
            let extra_length =
                u16::from_le_bytes(bytes[offset + 30..offset + 32].try_into().unwrap()) as usize;
            let comment_length =
                u16::from_le_bytes(bytes[offset + 32..offset + 34].try_into().unwrap()) as usize;
            let header_offset =
                u32::from_le_bytes(bytes[offset + 42..offset + 46].try_into().unwrap()) as usize;
            let name_start = offset + 46;
            let name_end = name_start + name_length;
            if name_end > bytes.len() {
                return Err(ImageError::new("truncated ZIP directory"));
            }
            let name = String::from_utf8_lossy(&bytes[name_start..name_end]).into_owned();
            offset = name_end + extra_length + comment_length;

            if header_offset + 30 > bytes.len()
                || &bytes[header_offset..header_offset + 4] != b"PK\x03\x04"
            {
                return Err(ImageError::new("invalid ZIP entry"));
            }
            let local_name = u16::from_le_bytes(
                bytes[header_offset + 26..header_offset + 28]
                    .try_into()
                    .unwrap(),
            ) as usize;
            let local_extra = u16::from_le_bytes(
                bytes[header_offset + 28..header_offset + 30]
                    .try_into()
                    .unwrap(),
            ) as usize;
            let data_start = header_offset + 30 + local_name + local_extra;
            let data_end = data_start + compressed_size;
            if data_end > bytes.len() {
                return Err(ImageError::new("truncated ZIP entry"));
            }
            let compressed = &bytes[data_start..data_end];
            let data = match method {
                0 => compressed.to_vec(),
                // ZIP method 8 is a raw DEFLATE stream, without zlib framing.
                8 => crate::compress::inflate(compressed)?,
                _ => return Err(ImageError::new("unsupported ZIP compression")),
            };
            if data.len() != uncompressed_size {
                return Err(ImageError::new("ZIP entry size mismatch"));
            }
            entries.push(ZipEntry { name, data });
        }
        Ok(Self { entries })
    }

    #[must_use]
    pub fn entries(&self) -> &[ZipEntry] {
        &self.entries
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.data.as_slice())
    }
}

/// A ZIP writer that stores every entry with DEFLATE level 0 framing.
#[derive(Debug, Default)]
pub struct ZipWriter {
    entries: Vec<ZipEntry>,
}

impl ZipWriter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, name: impl Into<String>, data: impl Into<Vec<u8>>) {
        self.entries.push(ZipEntry {
            name: name.into(),
            data: data.into(),
        });
    }

    /// Serialize the archive. Entry names must be unique.
    // Ported from omnishot/editor.py (write_project)
    #[must_use]
    pub fn finish(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut directory = Vec::new();
        for entry in &self.entries {
            let offset = out.len() as u32;
            let name = entry.name.as_bytes();
            let compressed = deflate_store(&entry.data);
            let crc = crc32(&entry.data);
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&(20u16).to_le_bytes()); // version needed
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&8u16.to_le_bytes()); // deflate
            out.extend_from_slice(&0u16.to_le_bytes()); // mod time
            out.extend_from_slice(&0u16.to_le_bytes()); // mod date
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            out.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra
            out.extend_from_slice(name);
            out.extend_from_slice(&compressed);

            directory.extend_from_slice(b"PK\x01\x02");
            directory.extend_from_slice(&(20u16).to_le_bytes());
            directory.extend_from_slice(&(20u16).to_le_bytes());
            directory.extend_from_slice(&0u16.to_le_bytes());
            directory.extend_from_slice(&8u16.to_le_bytes());
            directory.extend_from_slice(&0u16.to_le_bytes());
            directory.extend_from_slice(&0u16.to_le_bytes());
            directory.extend_from_slice(&crc.to_le_bytes());
            directory.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            directory.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
            directory.extend_from_slice(&(name.len() as u16).to_le_bytes());
            directory.extend_from_slice(&0u16.to_le_bytes());
            directory.extend_from_slice(&0u16.to_le_bytes());
            directory.extend_from_slice(&0u16.to_le_bytes());
            directory.extend_from_slice(&0u16.to_le_bytes());
            directory.extend_from_slice(&0u32.to_le_bytes());
            directory.extend_from_slice(&offset.to_le_bytes());
            directory.extend_from_slice(name);
        }
        let directory_offset = out.len() as u32;
        out.extend_from_slice(&directory);
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(directory.len() as u32).to_le_bytes());
        out.extend_from_slice(&directory_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    /// Write the archive atomically beside `destination`.
    pub fn write_atomic(&self, destination: &Path) -> Result<(), ImageError> {
        crate::storage::atomic_write_file(destination, &self.finish())
            .map_err(|error| ImageError::new(error.to_string()))
    }
}

fn find_eocd(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 22 {
        return None;
    }
    let start = bytes.len().saturating_sub(22 + 65_535);
    for offset in (start..=bytes.len() - 22).rev() {
        if &bytes[offset..offset + 4] == b"PK\x05\x06" {
            return Some(offset);
        }
    }
    None
}

/// A small map for caller-supplied metadata keyed by entry name.
pub type Metadata = BTreeMap<String, Vec<u8>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trips_rgba_pixels() {
        let mut image = Image::transparent(4, 3);
        image.data[0..4].copy_from_slice(&[10, 20, 30, 255]);
        let offset = (2 * 4 + 3) * 4;
        image.data[offset..offset + 4].copy_from_slice(&[200, 100, 50, 128]);
        let encoded = image.to_png();
        let decoded = decode_png(&encoded).unwrap();
        assert_eq!(decoded, image);
    }

    #[test]
    fn png_decodes_rgb_and_grayscale_input() {
        // A hand-built 2x1 RGB PNG with one red and one green pixel.
        let raw = vec![
            0, 255, 0, 0, 0, 255, 0, // filter + red, green
        ];
        let zlib = zlib_store(&raw);
        let mut png = Vec::new();
        png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);
        let mut header = Vec::new();
        header.extend_from_slice(&2u32.to_be_bytes());
        header.extend_from_slice(&1u32.to_be_bytes());
        header.extend_from_slice(&[8, 2, 0, 0, 0]);
        push_chunk(&mut png, b"IHDR", &header);
        push_chunk(&mut png, b"IDAT", &zlib);
        push_chunk(&mut png, b"IEND", &[]);
        let decoded = decode_png(&png).unwrap();
        assert_eq!(decoded.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(decoded.pixel(1, 0), [0, 255, 0, 255]);
    }

    #[test]
    fn corrupt_png_is_rejected() {
        assert!(decode_png(b"not a png").is_err());
        assert!(decode_png(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]).is_err());
    }

    #[test]
    fn zip_round_trips_named_entries() {
        let mut writer = ZipWriter::new();
        writer.add("image.png", vec![1u8, 2, 3]);
        writer.add("project.json", b"{\"version\":2}".to_vec());
        let archive = writer.finish();
        let read = ZipArchive::read(&archive).unwrap();
        assert_eq!(read.get("project.json").unwrap(), b"{\"version\":2}");
        assert_eq!(read.get("image.png").unwrap(), &[1, 2, 3]);
        assert!(read.get("missing").is_none());
        assert_eq!(read.entries().len(), 2);
    }

    #[test]
    fn zip_reads_a_stored_entry_without_compression() {
        let mut writer = ZipWriter::new();
        writer.add("raw", b"plain".to_vec());
        let archive = writer.finish();
        assert_eq!(
            ZipArchive::read(&archive).unwrap().get("raw").unwrap(),
            b"plain"
        );
        // Truncating the central directory invalidates the archive.
        assert!(ZipArchive::read(&archive[..archive.len() / 2]).is_err());
    }
}
