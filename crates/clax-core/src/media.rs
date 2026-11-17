//! What an image file is, read from its bytes: the media type and the
//! displayed size in pixels of a PNG, JPEG, WebP or GIF, from the file's
//! header alone: a JPEG's after its EXIF orientation, a GIF's the canvas a
//! decoder draws its first frame on. Nothing is decoded, nothing is
//! allocated, and the work done on any input is bounded (see [`sniff`]).

/// The media type of a PNG.
pub const PNG: &str = "image/png";
/// The media type of a JPEG.
pub const JPEG: &str = "image/jpeg";
/// The media type of a WebP.
pub const WEBP: &str = "image/webp";
/// The media type of a GIF.
pub const GIF: &str = "image/gif";

/// The most segments [`sniff`] reads of a JPEG before its frame header; a
/// JPEG with more is refused.
pub const JPEG_SEGMENTS_MAX: usize = 1024;

/// The most fill bytes (`FF`) [`sniff`] skips before one JPEG marker; a
/// JPEG with more is refused.
pub const JPEG_FILL_MAX: usize = 64;

/// The most entries [`sniff`] reads of the first IFD of a JPEG's EXIF
/// block looking for its Orientation; one past them is not read.
pub const EXIF_ENTRIES_MAX: usize = 256;

/// The most blocks [`sniff`] reads of a GIF past its global colour table
/// looking for its first image descriptor (an extension's introducer and
/// each of its sub-blocks count one each); one past them is not read.
pub const GIF_BLOCKS_MAX: usize = 1024;

/// What [`sniff`] found: one of [`PNG`], [`JPEG`], [`WEBP`] and [`GIF`],
/// and the image's displayed size in pixels, neither side zero: a JPEG's
/// after its EXIF orientation, a GIF's the canvas its first frame is
/// drawn on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sniffed {
    pub media_type: &'static str,
    pub width: u32,
    pub height: u32,
}

/// Why [`sniff`] refused bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SniffError {
    /// The bytes do not start as any of the four formats does.
    #[error("not a PNG, JPEG, WebP or GIF image")]
    Unrecognized,
    /// The bytes start as `media_type` does and end before its header
    /// does, or before a length the header declares.
    #[error("a truncated {media_type} file")]
    Truncated { media_type: &'static str },
    /// The bytes start as `media_type` does and their header is not one
    /// the format allows; `why` says what is wrong, as the rest of a
    /// sentence.
    #[error("not a valid {media_type} file: {why}")]
    Invalid {
        media_type: &'static str,
        why: &'static str,
    },
}

/// The media type and displayed size in pixels of the image `bytes`
/// holds, the whole file or a prefix of it that reaches as far as
/// described here.
///
/// The format is decided by the leading signature alone, and then the
/// header must hold:
///
/// - **PNG**: the signature and a whole `IHDR` chunk first (33 bytes), of
///   length 13 and with a correct CRC; each side from 1 to 2^31 - 1.
/// - **GIF**: `GIF87a` or `GIF89a` and the whole logical screen descriptor
///   (13 bytes), the logical screen's sides neither zero. Each side is the
///   larger of the logical screen's and the first frame's extent (its
///   left + width, its top + height), since a decoder enlarges the canvas
///   to hold that frame. The first frame is the first image descriptor
///   (`2C`) past the global colour table and any extension blocks, when
///   `bytes` holds it whole within [`GIF_BLOCKS_MAX`] blocks; when it does
///   not, the sides are the logical screen's.
/// - **JPEG**: `FF D8`, then segments up to the first frame header (any
///   `SOF` marker), which must be whole, of the length its component count
///   gives, and with neither side zero. Each segment before it must lie
///   whole within `bytes`. A file whose frame header is not among its first
///   [`JPEG_SEGMENTS_MAX`] segments, or that has more than
///   [`JPEG_FILL_MAX`] fill bytes before a marker, is refused. The sides
///   are the frame's, swapped when the first `APP1` segment that starts
///   `Exif\0\0`, if it comes before the frame header, holds a TIFF header
///   whose first IFD has an Orientation (tag `0112`, one `SHORT`) of 5,
///   6, 7 or 8 among its first [`EXIF_ENTRIES_MAX`] entries. An EXIF block
///   that is malformed or runs past its segment, and an Orientation out of
///   1 to 8, are read as no orientation, never as a refusal.
/// - **WebP**: `RIFF`, a size, `WEBP`, and a first chunk that is `VP8 `
///   (a key frame, neither side zero), `VP8L` (version 0) or `VP8X` (of
///   length 10, the canvas at most 2^32 - 1 pixels); the first chunk must
///   lie within the size the `RIFF` header declares, and that size within
///   `bytes`. For `VP8X` the dimensions are the canvas's.
///
/// Pixel data, later chunks and bytes after those named are not read: a
/// PNG's `eXIf` chunk and a WebP's `EXIF` chunk are not, so a PNG's and a
/// WebP's sides are the stored ones whatever orientation those state.
/// The work is bounded by a constant, whatever `bytes` holds, and no input
/// panics.
pub fn sniff(bytes: &[u8]) -> Result<Sniffed, SniffError> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(bytes)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        gif(bytes)
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        jpeg(bytes)
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        webp(bytes)
    } else {
        Err(SniffError::Unrecognized)
    }
}

/// The `N` bytes of `bytes` from `at`; `None` when it ends before them.
fn take<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..at.checked_add(N)?)?.try_into().ok()
}

fn truncated(media_type: &'static str) -> SniffError {
    SniffError::Truncated { media_type }
}

fn invalid(media_type: &'static str, why: &'static str) -> SniffError {
    SniffError::Invalid { media_type, why }
}

/// A [`Sniffed`] of `media_type`, or the refusal of a zero side.
fn sized(media_type: &'static str, width: u32, height: u32) -> Result<Sniffed, SniffError> {
    if width == 0 || height == 0 {
        return Err(invalid(media_type, "its width or height is zero"));
    }
    Ok(Sniffed {
        media_type,
        width,
        height,
    })
}

/// The CRC-32 a PNG chunk carries, of `bytes`.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn png(bytes: &[u8]) -> Result<Sniffed, SniffError> {
    // The signature (8), then the chunk: length (4), type (4), data (13),
    // CRC (4).
    let chunk: [u8; 25] = take(bytes, 8).ok_or(truncated(PNG))?;
    let [length, typed, crc]: [&[u8]; 3] = [&chunk[..4], &chunk[4..21], &chunk[21..]];
    if &typed[..4] != b"IHDR" {
        return Err(invalid(PNG, "its first chunk is not IHDR"));
    }
    if length != [0, 0, 0, 13] {
        return Err(invalid(PNG, "its IHDR chunk is not 13 bytes long"));
    }
    if crc != crc32(typed).to_be_bytes() {
        return Err(invalid(PNG, "its IHDR chunk's CRC is wrong"));
    }
    let be = |at: usize| take(typed, at).map(u32::from_be_bytes).unwrap_or(0);
    let (width, height) = (be(4), be(8));
    if width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(invalid(PNG, "its width or height is over 2^31 - 1"));
    }
    sized(PNG, width, height)
}

fn gif(bytes: &[u8]) -> Result<Sniffed, SniffError> {
    // The signature (6), then the logical screen descriptor: width (2),
    // height (2), flags, background, aspect ratio.
    let screen: [u8; 7] = take(bytes, 6).ok_or(truncated(GIF))?;
    let width = u16::from_le_bytes([screen[0], screen[1]]);
    let height = u16::from_le_bytes([screen[2], screen[3]]);
    let screen_size = sized(GIF, width.into(), height.into())?;
    let (right, bottom) = gif_first_frame(bytes, screen[4]).unwrap_or((0, 0));
    sized(
        GIF,
        screen_size.width.max(right),
        screen_size.height.max(bottom),
    )
}

/// The right and bottom edges of a GIF's first frame (its left + width,
/// its top + height), given the logical screen's flags; `None` when
/// `bytes` does not hold its image descriptor whole within
/// [`GIF_BLOCKS_MAX`] blocks, or a block that is neither an extension nor
/// an image descriptor comes first.
fn gif_first_frame(bytes: &[u8], flags: u8) -> Option<(u32, u32)> {
    // The header (6) and the logical screen descriptor (7), then the
    // global colour table when the flags' top bit says there is one: 3
    // bytes for each of 2^(n + 1) colours, n the flags' low 3 bits.
    let mut at: usize = 13;
    if flags & 0x80 != 0 {
        at += 3 << ((flags & 0x07) + 1);
    }
    let mut in_extension = false;
    for _ in 0..GIF_BLOCKS_MAX {
        if in_extension {
            // A sub-block: its length, then that many bytes; a length of
            // zero ends the extension.
            let [length]: [u8; 1] = take(bytes, at)?;
            at = at.checked_add(1 + usize::from(length))?;
            in_extension = length != 0;
            continue;
        }
        let [introducer]: [u8; 1] = take(bytes, at)?;
        match introducer {
            // The introducer, then left (2), top (2), width (2) and height
            // (2), little-endian, and the frame's flags.
            0x2c => {
                let [l0, l1, t0, t1, w0, w1, h0, h1, _]: [u8; 9] = take(bytes, at + 1)?;
                let side = |a, b| u32::from(u16::from_le_bytes([a, b]));
                return Some((side(l0, l1) + side(w0, w1), side(t0, t1) + side(h0, h1)));
            }
            // The introducer and the extension's label, then sub-blocks.
            0x21 => {
                at = at.checked_add(2)?;
                in_extension = true;
            }
            _ => return None,
        }
    }
    None
}

/// The Orientation, 1 to 8, that the first IFD of the TIFF header `tiff`
/// (an EXIF block's, after `Exif\0\0`) holds among its first
/// [`EXIF_ENTRIES_MAX`] entries; `None` when it holds none, holds one
/// that is not one `SHORT` from 1 to 8, or `tiff` is malformed or ends
/// before the entry.
fn exif_orientation(tiff: &[u8]) -> Option<u16> {
    // The byte order (2), 42 (2), the first IFD's offset from the start
    // of `tiff` (4).
    let little = match &take::<2>(tiff, 0)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |at: usize| {
        take(tiff, at).map(|b| {
            if little {
                u16::from_le_bytes(b)
            } else {
                u16::from_be_bytes(b)
            }
        })
    };
    let u32_at = |at: usize| {
        take(tiff, at).map(|b| {
            if little {
                u32::from_le_bytes(b)
            } else {
                u32::from_be_bytes(b)
            }
        })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    // The IFD: its entry count (2), then 12 bytes an entry: tag (2), type
    // (2), count (4), and the value or its offset (4).
    let ifd = usize::try_from(u32_at(4)?).ok()?;
    let entries = usize::from(u16_at(ifd)?).min(EXIF_ENTRIES_MAX);
    for n in 0..entries {
        let entry = ifd.checked_add(2)?.checked_add(12 * n)?;
        if u16_at(entry)? != 0x0112 {
            continue;
        }
        // Type 3 is SHORT; one fits the value's field, at its start.
        if u16_at(entry.checked_add(2)?)? != 3 || u32_at(entry.checked_add(4)?)? != 1 {
            return None;
        }
        return u16_at(entry.checked_add(8)?).filter(|o| (1..=8).contains(o));
    }
    None
}

fn jpeg(bytes: &[u8]) -> Result<Sniffed, SniffError> {
    let mut at = 2;
    // The first EXIF block's Orientation, once one is seen.
    let mut orientation: Option<Option<u16>> = None;
    for _ in 0..JPEG_SEGMENTS_MAX {
        let [lead]: [u8; 1] = take(bytes, at).ok_or(truncated(JPEG))?;
        if lead != 0xff {
            return Err(invalid(JPEG, "a segment does not start with a marker"));
        }
        at += 1;
        let mut fill = 0;
        let marker = loop {
            let [byte]: [u8; 1] = take(bytes, at).ok_or(truncated(JPEG))?;
            at += 1;
            if byte != 0xff {
                break byte;
            }
            fill += 1;
            if fill > JPEG_FILL_MAX {
                return Err(invalid(JPEG, "too many fill bytes before a marker"));
            }
        };
        match marker {
            0x00 => return Err(invalid(JPEG, "a segment does not start with a marker")),
            // TEM and the restart markers stand alone.
            0x01 | 0xd0..=0xd7 => continue,
            0xd8 => return Err(invalid(JPEG, "it starts a second image")),
            0xd9 => return Err(invalid(JPEG, "it ends before a frame header")),
            0xda => return Err(invalid(JPEG, "a scan comes before a frame header")),
            _ => {}
        }
        let length = usize::from(u16::from_be_bytes(take(bytes, at).ok_or(truncated(JPEG))?));
        if length < 2 {
            return Err(invalid(JPEG, "a segment is shorter than its own length"));
        }
        let segment = bytes.get(at..at + length).ok_or(truncated(JPEG))?;
        // Every SOF but DHT (C4), JPG (C8) and DAC (CC), which share the
        // range.
        if matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            // Length (2), precision, height (2), width (2), the component
            // count, three bytes a component.
            let Some([_, _, _, h0, h1, w0, w1, components]) = take::<8>(segment, 0) else {
                return Err(invalid(JPEG, "its frame header is too short"));
            };
            if components == 0 || length != 8 + 3 * usize::from(components) {
                return Err(invalid(
                    JPEG,
                    "its frame header's length is not its components'",
                ));
            }
            let (width, height) = (u16::from_be_bytes([w0, w1]), u16::from_be_bytes([h0, h1]));
            // Orientations 5 to 8 transpose the stored image.
            return if matches!(orientation, Some(Some(5..=8))) {
                sized(JPEG, height.into(), width.into())
            } else {
                sized(JPEG, width.into(), height.into())
            };
        }
        // APP1: its length (2), then an EXIF block is `Exif\0\0` and a
        // TIFF header.
        if marker == 0xe1
            && orientation.is_none()
            && let Some(tiff) = segment.get(2..).and_then(|p| p.strip_prefix(b"Exif\0\0"))
        {
            orientation = Some(exif_orientation(tiff));
        }
        at += length;
    }
    Err(invalid(JPEG, "too many segments before a frame header"))
}

fn webp(bytes: &[u8]) -> Result<Sniffed, SniffError> {
    // RIFF (4), the size of what follows it (4), WEBP (4), then chunks: a
    // name (4), the payload's length (4), the payload.
    let riff = u64::from(u32::from_le_bytes(take(bytes, 4).ok_or(truncated(WEBP))?));
    let name: [u8; 4] = take(bytes, 12).ok_or(truncated(WEBP))?;
    let length = u64::from(u32::from_le_bytes(take(bytes, 16).ok_or(truncated(WEBP))?));
    // WEBP and the chunk's header are 12 of the bytes `riff` counts.
    if 12 + length > riff {
        return Err(invalid(
            WEBP,
            "its first chunk runs past the size its RIFF header declares",
        ));
    }
    if 8 + riff > bytes.len() as u64 {
        return Err(truncated(WEBP));
    }
    // Within `bytes`, by the two checks above.
    let payload = bytes
        .get(20..)
        .and_then(|rest| rest.get(..usize::try_from(length).ok()?))
        .ok_or(truncated(WEBP))?;
    match &name {
        b"VP8 " => {
            // The frame tag (3), the start code (3), then each side in 14
            // bits under 2 of scaling.
            let Some([tag, _, _, s0, s1, s2, w0, w1, h0, h1]) = take::<10>(payload, 0) else {
                return Err(invalid(WEBP, "its VP8 chunk is too short"));
            };
            if tag & 1 != 0 {
                return Err(invalid(WEBP, "its VP8 chunk is not a key frame"));
            }
            if [s0, s1, s2] != [0x9d, 0x01, 0x2a] {
                return Err(invalid(WEBP, "its VP8 chunk has no start code"));
            }
            sized(
                WEBP,
                (u16::from_le_bytes([w0, w1]) & 0x3fff).into(),
                (u16::from_le_bytes([h0, h1]) & 0x3fff).into(),
            )
        }
        b"VP8L" => {
            // The signature, then 14 bits of each side less one, a bit of
            // alpha and 3 of version.
            let Some([signature, b0, b1, b2, b3]) = take::<5>(payload, 0) else {
                return Err(invalid(WEBP, "its VP8L chunk is too short"));
            };
            if signature != 0x2f {
                return Err(invalid(WEBP, "its VP8L chunk has no signature"));
            }
            let bits = u32::from_le_bytes([b0, b1, b2, b3]);
            if bits >> 29 != 0 {
                return Err(invalid(WEBP, "its VP8L chunk is not version 0"));
            }
            sized(WEBP, (bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
        }
        b"VP8X" => {
            // Flags, 3 reserved bytes, then 24 bits of each side less one.
            if length != 10 {
                return Err(invalid(WEBP, "its VP8X chunk is not 10 bytes long"));
            }
            let Some([_, _, _, _, w0, w1, w2, h0, h1, h2]) = take::<10>(payload, 0) else {
                return Err(truncated(WEBP));
            };
            let width = u32::from_le_bytes([w0, w1, w2, 0]) + 1;
            let height = u32::from_le_bytes([h0, h1, h2, 0]) + 1;
            if u64::from(width) * u64::from(height) > u64::from(u32::MAX) {
                return Err(invalid(WEBP, "its canvas is over 2^32 - 1 pixels"));
            }
            sized(WEBP, width, height)
        }
        _ => Err(invalid(WEBP, "its first chunk is not VP8, VP8L or VP8X")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(media_type: &'static str, width: u32, height: u32) -> Result<Sniffed, SniffError> {
        Ok(Sniffed {
            media_type,
            width,
            height,
        })
    }

    fn bad(media_type: &'static str, why: &'static str) -> Result<Sniffed, SniffError> {
        Err(invalid(media_type, why))
    }

    fn cut(media_type: &'static str) -> Result<Sniffed, SniffError> {
        Err(truncated(media_type))
    }

    /// A PNG's signature and `IHDR` chunk, then the start of an `IDAT`.
    fn png_file(width: u32, height: u32) -> Vec<u8> {
        let mut typed = b"IHDR".to_vec();
        typed.extend(width.to_be_bytes());
        typed.extend(height.to_be_bytes());
        typed.extend([8, 6, 0, 0, 0]);
        let mut file = b"\x89PNG\r\n\x1a\n\0\0\0\x0d".to_vec();
        file.extend(&typed);
        file.extend(crc32(&typed).to_be_bytes());
        file.extend(b"\0\0\0\x0aIDAT");
        file
    }

    /// A GIF's header and logical screen descriptor, then a trailer.
    fn gif_file(version: &[u8; 6], width: u16, height: u16) -> Vec<u8> {
        let mut file = version.to_vec();
        file.extend(width.to_le_bytes());
        file.extend(height.to_le_bytes());
        file.extend([0, 0, 0, 0x3b]);
        file
    }

    /// One JPEG segment: `marker`, its length, `payload`.
    fn segment(marker: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xff, marker];
        bytes.extend(u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
        bytes.extend(payload);
        bytes
    }

    /// A frame header's payload: 8 bits, `height`, `width`, one component.
    fn frame(width: u16, height: u16) -> Vec<u8> {
        let mut payload = vec![8];
        payload.extend(height.to_be_bytes());
        payload.extend(width.to_be_bytes());
        payload.extend([1, 1, 0x11, 0]);
        payload
    }

    /// A JPEG: `SOI`, an `APP0`, a quantization table's segment, a
    /// restart marker, a frame header under `marker` after two fill bytes,
    /// then the start of a scan. The second value is where the frame
    /// header ends.
    fn jpeg_file(marker: u8, width: u16, height: u16) -> (Vec<u8>, usize) {
        let mut file = vec![0xff, 0xd8];
        file.extend(segment(0xe0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"));
        file.extend(segment(0xdb, &[0; 65]));
        file.extend([0xff, 0xd0, 0xff, 0xff]);
        file.extend(segment(marker, &frame(width, height)));
        let end = file.len();
        file.extend(segment(0xda, &[1, 1, 0, 0, 0x3f, 0]));
        file.extend([0x12, 0x34]);
        (file, end)
    }

    /// A JPEG: `SOI`, the segments `before`, a baseline frame header,
    /// then the start of a scan. The second value is where the frame
    /// header ends.
    fn jpeg_with(before: &[&[u8]], width: u16, height: u16) -> (Vec<u8>, usize) {
        let mut file = vec![0xff, 0xd8];
        file.extend(before.concat());
        file.extend(segment(0xc0, &frame(width, height)));
        let end = file.len();
        file.extend(segment(0xda, &[1, 1, 0, 0, 0x3f, 0]));
        (file, end)
    }

    /// One IFD entry: tag, type, count, and a `SHORT` value.
    type Entry = (u16, u16, u32, u16);

    /// A TIFF header in byte order `order` (`II` or `MM`) whose first
    /// IFD, straight after it, holds `entries`.
    fn tiff(order: &[u8; 2], entries: &[Entry]) -> Vec<u8> {
        let little = order == b"II";
        let two = |v: u16| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let four = |v: u32| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let mut t = order.to_vec();
        t.extend(two(42));
        t.extend(four(8));
        t.extend(two(u16::try_from(entries.len()).unwrap()));
        for &(tag, typ, count, value) in entries {
            t.extend(two(tag));
            t.extend(two(typ));
            t.extend(four(count));
            t.extend(two(value));
            t.extend([0, 0]);
        }
        // The next IFD's offset: none.
        t.extend([0; 4]);
        t
    }

    /// A TIFF header as a camera writes one: a make, the Orientation
    /// `value`, a resolution.
    fn oriented(order: &[u8; 2], value: u16) -> Vec<u8> {
        tiff(
            order,
            &[(0x010f, 2, 4, 0), (0x0112, 3, 1, value), (0x011a, 5, 1, 0)],
        )
    }

    /// An `APP1` segment of an EXIF block holding `tiff`.
    fn exif(tiff: &[u8]) -> Vec<u8> {
        segment(0xe1, &[&b"Exif\0\0"[..], tiff].concat())
    }

    /// A JPEG of 640x480 stored, after a JFIF `APP0` and `app1`.
    fn photo(app1: &[u8]) -> Vec<u8> {
        jpeg_with(
            &[&segment(0xe0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0"), app1],
            640,
            480,
        )
        .0
    }

    /// A GIF: `version`, a logical screen of `width`x`height` under
    /// `flags`, `between` (a colour table, extensions), then an image
    /// descriptor of `frame` (left, top, width, height) and its data,
    /// and a trailer. The second value is where the image descriptor
    /// ends.
    fn gif_with(
        version: &[u8; 6],
        (width, height, flags): (u16, u16, u8),
        between: &[u8],
        frame: (u16, u16, u16, u16),
    ) -> (Vec<u8>, usize) {
        let mut file = version.to_vec();
        file.extend(width.to_le_bytes());
        file.extend(height.to_le_bytes());
        file.extend([flags, 0, 0]);
        file.extend(between);
        file.push(0x2c);
        for side in [frame.0, frame.1, frame.2, frame.3] {
            file.extend(side.to_le_bytes());
        }
        file.push(0);
        let end = file.len();
        file.extend([2, 2, 0x44, 0x01, 0, 0x3b]);
        (file, end)
    }

    /// A graphic control, a comment and a NETSCAPE looping extension.
    fn gif_extensions() -> Vec<u8> {
        [
            &[0x21, 0xf9, 4, 0, 10, 0, 0, 0][..],
            &[0x21, 0xfe, 3, b'a', b'b', b'c', 0],
            &[0x21, 0xff, 11],
            b"NETSCAPE2.0",
            &[3, 1, 0, 0, 0],
        ]
        .concat()
    }

    /// A width and a height.
    type Size = (u32, u32);

    /// GIFs whose first frame grows the canvas: each file, where its
    /// image descriptor ends, the logical screen's size and the canvas's.
    fn gif_frame_files() -> Vec<(Vec<u8>, usize, Size, Size)> {
        let wide = gif_with(b"GIF89a", (10, 10, 0), &[], (0, 0, 20, 5));
        let tall = gif_with(b"GIF89a", (10, 10, 0), &[], (0, 0, 5, 30));
        let offset = gif_with(b"GIF89a", (10, 10, 0), &[], (8, 9, 5, 5));
        let extended = gif_with(b"GIF89a", (1, 1, 0), &gif_extensions(), (0, 0, 50, 40));
        // A colour table of 8 colours, each byte an image descriptor's
        // introducer, so that a walk that misplaces its end finds another
        // frame or none.
        let coloured = gif_with(b"GIF89a", (4, 4, 0x82), &[0x2c; 24], (0, 0, 30, 30));
        let old = gif_with(b"GIF87a", (3, 3, 0), &[], (1, 1, 7, 2));
        vec![
            (wide.0, wide.1, (10, 10), (20, 10)),
            (tall.0, tall.1, (10, 10), (10, 30)),
            (offset.0, offset.1, (10, 10), (13, 14)),
            (extended.0, extended.1, (1, 1), (50, 40)),
            (coloured.0, coloured.1, (4, 4), (30, 30)),
            (old.0, old.1, (3, 3), (8, 3)),
        ]
    }

    /// A WebP of one chunk, `name` holding `payload`, padded to an even
    /// length as RIFF has it.
    fn webp_file(name: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let padded = payload.len() + payload.len() % 2;
        let mut file = b"RIFF".to_vec();
        file.extend(u32::try_from(12 + padded).unwrap().to_le_bytes());
        file.extend(b"WEBP");
        file.extend(name);
        file.extend(u32::try_from(payload.len()).unwrap().to_le_bytes());
        file.extend(payload);
        file.resize(20 + padded, 0);
        file
    }

    fn vp8(width: u16, height: u16) -> Vec<u8> {
        let mut payload = vec![0x10, 0x02, 0x00, 0x9d, 0x01, 0x2a];
        payload.extend(width.to_le_bytes());
        payload.extend(height.to_le_bytes());
        payload
    }

    fn vp8l(width: u32, height: u32, version: u32) -> Vec<u8> {
        let mut payload = vec![0x2f];
        payload.extend(
            ((width - 1) | ((height - 1) << 14) | (1 << 28) | (version << 29)).to_le_bytes(),
        );
        payload
    }

    fn vp8x(width: u32, height: u32) -> Vec<u8> {
        let mut payload = vec![0x10, 0, 0, 0];
        payload.extend(&(width - 1).to_le_bytes()[..3]);
        payload.extend(&(height - 1).to_le_bytes()[..3]);
        payload
    }

    /// Each format's minimal file, the bytes of it [`sniff`] needs, and
    /// what it finds.
    fn files() -> Vec<(Vec<u8>, usize, Sniffed)> {
        let is = |media_type, width, height| Sniffed {
            media_type,
            width,
            height,
        };
        let (baseline, baseline_end) = jpeg_file(0xc0, 640, 480);
        let (progressive, progressive_end) = jpeg_file(0xc2, 65535, 1);
        let whole = |file: Vec<u8>, found| (file.clone(), file.len(), found);
        let app0 = segment(0xe0, b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        let (turned, turned_end) = jpeg_with(&[&app0, &exif(&oriented(b"II", 6))], 640, 480);
        let (flipped, flipped_end) = jpeg_with(&[&exif(&oriented(b"MM", 7)), &app0], 3, 2);
        vec![
            (png_file(800, 600), 33, is(PNG, 800, 600)),
            (gif_file(b"GIF89a", 320, 200), 13, is(GIF, 320, 200)),
            (gif_file(b"GIF87a", 1, 65535), 13, is(GIF, 1, 65535)),
            (baseline, baseline_end, is(JPEG, 640, 480)),
            (progressive, progressive_end, is(JPEG, 65535, 1)),
            (turned, turned_end, is(JPEG, 480, 640)),
            (flipped, flipped_end, is(JPEG, 2, 3)),
            whole(webp_file(b"VP8 ", &vp8(176, 144)), is(WEBP, 176, 144)),
            whole(webp_file(b"VP8L", &vp8l(16384, 3, 0)), is(WEBP, 16384, 3)),
            whole(webp_file(b"VP8X", &vp8x(70000, 2)), is(WEBP, 70000, 2)),
        ]
    }

    #[test]
    fn each_format_is_found_with_its_dimensions() {
        for (file, _, expected) in files() {
            assert_eq!(sniff(&file), Ok(expected));
        }
    }

    #[test]
    fn a_known_png_header_is_found() {
        // The first 33 bytes of a 1x1 RGBA PNG, its CRC as an encoder
        // wrote it.
        let file =
            b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89";
        assert_eq!(sniff(file), found(PNG, 1, 1));
    }

    #[test]
    fn every_prefix_is_refused_until_the_header_is_whole() {
        for (file, need, expected) in files() {
            for length in 0..need {
                let result = sniff(&file[..length]);
                assert!(
                    matches!(
                        result,
                        Err(SniffError::Unrecognized | SniffError::Truncated { .. })
                    ),
                    "{} bytes of a {}: {result:?}",
                    length,
                    expected.media_type
                );
            }
            for length in need..=file.len() {
                assert_eq!(sniff(&file[..length]), Ok(expected), "{length} bytes");
            }
            let mut longer = file.clone();
            longer.extend(b"trailing");
            assert_eq!(sniff(&longer), Ok(expected));
        }
    }

    #[test]
    fn a_prefix_past_the_signature_is_truncated_by_name() {
        assert_eq!(sniff(&png_file(2, 2)[..32]), cut(PNG));
        assert_eq!(sniff(&gif_file(b"GIF89a", 2, 2)[..12]), cut(GIF));
        let (jpeg, end) = jpeg_file(0xc0, 2, 2);
        assert_eq!(sniff(&jpeg[..end - 1]), cut(JPEG));
        assert_eq!(sniff(&jpeg[..3]), cut(JPEG));
        let webp = webp_file(b"VP8 ", &vp8(2, 2));
        assert_eq!(sniff(&webp[..webp.len() - 1]), cut(WEBP));
        assert_eq!(sniff(&webp[..12]), cut(WEBP));
    }

    #[test]
    fn a_gif_prefix_is_its_screen_until_its_first_frame_is_whole() {
        for (file, end, (sw, sh), (cw, ch)) in gif_frame_files() {
            for length in 0..13 {
                assert!(
                    matches!(
                        sniff(&file[..length]),
                        Err(SniffError::Unrecognized | SniffError::Truncated { .. })
                    ),
                    "{length} bytes"
                );
            }
            for length in 13..end {
                assert_eq!(sniff(&file[..length]), found(GIF, sw, sh), "{length} bytes");
            }
            for length in end..=file.len() {
                assert_eq!(sniff(&file[..length]), found(GIF, cw, ch), "{length} bytes");
            }
        }
    }

    #[test]
    fn no_changed_byte_panics() {
        let gifs = gif_frame_files().into_iter().map(|(file, ..)| file);
        let photos = [
            photo(&exif(&oriented(b"MM", 6))),
            photo(&exif(&tiff(b"II", &[(0x0112, 3, 1, 8); 3]))),
        ];
        let all: Vec<Vec<u8>> = files()
            .into_iter()
            .map(|(file, ..)| file)
            .chain(gifs)
            .chain(photos)
            .collect();
        for file in all {
            for at in 0..file.len() {
                for value in [0x00, 0x01, 0x7f, 0x80, 0xc0, 0xd8, 0xd9, 0xda, 0xfe, 0xff] {
                    let mut changed = file.clone();
                    changed[at] = value;
                    for length in 0..=changed.len() {
                        let _ = sniff(&changed[..length]);
                    }
                }
            }
        }
    }

    #[test]
    fn other_bytes_are_unrecognized() {
        for bytes in [
            &b""[..],
            b"\x89PNG\r\n\x1a",
            b"\x89PNG\r\n\x1a\r\0\0\0\x0dIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89",
            b"GIF88a\x01\0\x01\0\0\0\0",
            b"GIF8",
            b"\xff\xd8",
            b"\xff\xd8\x00\xff\xc0",
            b"RIFF\x16\0\0\0WAVEfmt \x0a\0\0\0",
            b"RIFF\x16\0\0\0WEB",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
            b"%PDF-1.7",
        ] {
            assert_eq!(sniff(bytes), Err(SniffError::Unrecognized), "{bytes:?}");
        }
    }

    #[test]
    fn a_png_whose_header_lies_is_refused() {
        let good = png_file(800, 600);

        let mut first = good.clone();
        first[12..16].copy_from_slice(b"IDAT");
        assert_eq!(sniff(&first), bad(PNG, "its first chunk is not IHDR"));

        let mut length = good.clone();
        length[11] = 12;
        assert_eq!(
            sniff(&length),
            bad(PNG, "its IHDR chunk is not 13 bytes long")
        );
        length[8..12].copy_from_slice(&[0xff; 4]);
        assert_eq!(
            sniff(&length),
            bad(PNG, "its IHDR chunk is not 13 bytes long")
        );

        // A width changed after the CRC was written.
        let mut edited = good.clone();
        edited[19] ^= 1;
        assert_eq!(sniff(&edited), bad(PNG, "its IHDR chunk's CRC is wrong"));

        for (width, height) in [(0, 600), (800, 0), (0, 0)] {
            assert_eq!(
                sniff(&png_file(width, height)),
                bad(PNG, "its width or height is zero")
            );
        }
        for (width, height) in [(1 << 31, 1), (1, u32::MAX)] {
            assert_eq!(
                sniff(&png_file(width, height)),
                bad(PNG, "its width or height is over 2^31 - 1")
            );
        }
        assert_eq!(
            sniff(&png_file(i32::MAX as u32, 1)),
            found(PNG, i32::MAX as u32, 1)
        );
    }

    #[test]
    fn a_gif_whose_header_lies_is_refused() {
        for (width, height) in [(0, 200), (320, 0), (0, 0)] {
            assert_eq!(
                sniff(&gif_file(b"GIF89a", width, height)),
                bad(GIF, "its width or height is zero")
            );
        }
    }

    #[test]
    fn a_jpeg_whose_header_lies_is_refused() {
        let soi = [0xff, 0xd8];
        let with = |parts: &[&[u8]]| [&soi[..], &parts.concat()].concat();

        for (width, height) in [(0, 480), (640, 0)] {
            assert_eq!(
                sniff(&jpeg_file(0xc0, width, height).0),
                bad(JPEG, "its width or height is zero")
            );
        }

        // A segment whose length runs past the end, before the frame
        // header and as the frame header.
        let mut long = segment(0xe1, &[0; 16]);
        long[2..4].copy_from_slice(&[0xff, 0xff]);
        assert_eq!(
            sniff(&with(&[&long, &segment(0xc0, &frame(1, 1))])),
            cut(JPEG)
        );
        let mut long = segment(0xc0, &frame(1, 1));
        long[2..4].copy_from_slice(&[0x00, 0x0e]);
        assert_eq!(sniff(&with(&[&long])), cut(JPEG));

        // A length that skips into the middle of what follows.
        let mut short = segment(0xe1, &[0; 16]);
        short[2..4].copy_from_slice(&[0x00, 0x04]);
        assert_eq!(
            sniff(&with(&[&short, &segment(0xc0, &frame(1, 1))])),
            bad(JPEG, "a segment does not start with a marker")
        );

        for length in [0, 1] {
            assert_eq!(
                sniff(&with(&[&[0xff, 0xe0, 0, length, 0, 0]])),
                bad(JPEG, "a segment is shorter than its own length")
            );
        }

        assert_eq!(
            sniff(&with(&[&[0xff, 0xc0, 0, 7, 8, 0, 1, 0, 1]])),
            bad(JPEG, "its frame header is too short")
        );
        // Three components declared, one present; and none declared.
        let mut components = frame(1, 1);
        components[5] = 3;
        assert_eq!(
            sniff(&with(&[&segment(0xc0, &components)])),
            bad(JPEG, "its frame header's length is not its components'")
        );
        assert_eq!(
            sniff(&with(&[&segment(0xc0, &[8, 0, 1, 0, 1, 0])])),
            bad(JPEG, "its frame header's length is not its components'")
        );

        let app = segment(0xe0, b"JFIF\0");
        assert_eq!(
            sniff(&with(&[&app, &[0xff, 0xd9]])),
            bad(JPEG, "it ends before a frame header")
        );
        assert_eq!(
            sniff(&with(&[&app, &segment(0xda, &[0; 6])])),
            bad(JPEG, "a scan comes before a frame header")
        );
        assert_eq!(
            sniff(&with(&[&app, &soi, &segment(0xc0, &frame(1, 1))])),
            bad(JPEG, "it starts a second image")
        );
        assert_eq!(
            sniff(&with(&[&app, &[0xff, 0x00]])),
            bad(JPEG, "a segment does not start with a marker")
        );
        assert_eq!(
            sniff(&with(&[&app, b"JFIF"])),
            bad(JPEG, "a segment does not start with a marker")
        );
    }

    #[test]
    fn a_jpeg_table_under_a_frame_headers_range_is_skipped() {
        // DHT, JPG and DAC, each holding what would read as 9x9.
        for marker in [0xc4, 0xc8, 0xcc] {
            let file = [
                &[0xff, 0xd8][..],
                &segment(marker, &frame(9, 9)),
                &segment(0xc1, &frame(3, 2)),
            ]
            .concat();
            assert_eq!(sniff(&file), found(JPEG, 3, 2), "{marker:#x}");
        }
    }

    #[test]
    fn a_jpeg_is_read_no_further_than_its_caps() {
        let frame = segment(0xc0, &frame(4, 4));

        // The frame header as the last segment allowed, and one later.
        let empty = segment(0xfe, &[]);
        let at = |before: usize| [&[0xff, 0xd8][..], &empty.repeat(before), &frame].concat();
        assert_eq!(sniff(&at(JPEG_SEGMENTS_MAX - 1)), found(JPEG, 4, 4));
        assert_eq!(
            sniff(&at(JPEG_SEGMENTS_MAX)),
            bad(JPEG, "too many segments before a frame header")
        );
        // Markers that stand alone count as segments too.
        let restarts = [&[0xff, 0xd8][..], &[0xff, 0xd0].repeat(1 << 20)].concat();
        assert_eq!(
            sniff(&restarts),
            bad(JPEG, "too many segments before a frame header")
        );

        // As many fill bytes as allowed, and one more. A marker's own FF
        // is not a fill byte, so the frame header's stands for one here.
        let filled = |fill: usize| [&[0xff, 0xd8][..], &vec![0xff; fill], &frame].concat();
        assert_eq!(sniff(&filled(JPEG_FILL_MAX)), found(JPEG, 4, 4));
        assert_eq!(
            sniff(&filled(JPEG_FILL_MAX + 1)),
            bad(JPEG, "too many fill bytes before a marker")
        );
        assert_eq!(sniff(&vec![0xff; 1 << 20]), Err(SniffError::Unrecognized));
        let mut all_fill = vec![0xff; 1 << 20];
        all_fill[1] = 0xd8;
        assert_eq!(
            sniff(&all_fill),
            bad(JPEG, "too many fill bytes before a marker")
        );
    }

    #[test]
    fn a_webp_whose_header_lies_is_refused() {
        let good = webp_file(b"VP8 ", &vp8(176, 144));

        // A RIFF size past the end of the file, and one short of the chunk.
        let mut riff = good.clone();
        riff[4] += 1;
        assert_eq!(sniff(&riff), cut(WEBP));
        riff[4..8].copy_from_slice(&[0xff; 4]);
        assert_eq!(sniff(&riff), cut(WEBP));
        riff[4..8].copy_from_slice(&21u32.to_le_bytes());
        assert_eq!(
            sniff(&riff),
            bad(
                WEBP,
                "its first chunk runs past the size its RIFF header declares"
            )
        );

        // A chunk length past the RIFF size.
        let mut chunk = good.clone();
        chunk[16..20].copy_from_slice(&[0xff; 4]);
        assert_eq!(
            sniff(&chunk),
            bad(
                WEBP,
                "its first chunk runs past the size its RIFF header declares"
            )
        );

        assert_eq!(
            sniff(&webp_file(b"ALPH", &[0; 10])),
            bad(WEBP, "its first chunk is not VP8, VP8L or VP8X")
        );

        assert_eq!(
            sniff(&webp_file(b"VP8 ", &vp8(1, 1)[..8])),
            bad(WEBP, "its VP8 chunk is too short")
        );
        let mut inter = vp8(1, 1);
        inter[0] |= 1;
        assert_eq!(
            sniff(&webp_file(b"VP8 ", &inter)),
            bad(WEBP, "its VP8 chunk is not a key frame")
        );
        let mut code = vp8(1, 1);
        code[4] = 0;
        assert_eq!(
            sniff(&webp_file(b"VP8 ", &code)),
            bad(WEBP, "its VP8 chunk has no start code")
        );
        for (width, height) in [(0, 144), (176, 0), (0xc000, 144)] {
            assert_eq!(
                sniff(&webp_file(b"VP8 ", &vp8(width, height))),
                bad(WEBP, "its width or height is zero")
            );
        }
        // The two bits of scaling above a side are not part of it.
        assert_eq!(
            sniff(&webp_file(b"VP8 ", &vp8(0x4000 | 176, 0xc000 | 144))),
            found(WEBP, 176, 144)
        );

        assert_eq!(
            sniff(&webp_file(b"VP8L", &vp8l(1, 1, 0)[..4])),
            bad(WEBP, "its VP8L chunk is too short")
        );
        let mut signature = vp8l(1, 1, 0);
        signature[0] = 0x2e;
        assert_eq!(
            sniff(&webp_file(b"VP8L", &signature)),
            bad(WEBP, "its VP8L chunk has no signature")
        );
        assert_eq!(
            sniff(&webp_file(b"VP8L", &vp8l(1, 1, 1))),
            bad(WEBP, "its VP8L chunk is not version 0")
        );

        for payload in [&vp8x(1, 1)[..9], &[vp8x(1, 1), vec![0, 0]].concat()] {
            assert_eq!(
                sniff(&webp_file(b"VP8X", payload)),
                bad(WEBP, "its VP8X chunk is not 10 bytes long")
            );
        }
        assert_eq!(
            sniff(&webp_file(b"VP8X", &vp8x(1 << 24, 1 << 8))),
            bad(WEBP, "its canvas is over 2^32 - 1 pixels")
        );
        assert_eq!(
            sniff(&webp_file(b"VP8X", &vp8x(1 << 24, 255))),
            found(WEBP, 1 << 24, 255)
        );
    }

    #[test]
    fn a_jpeg_is_sized_as_its_exif_orientation_shows_it() {
        for order in [b"II", b"MM"] {
            for value in 1..=8 {
                let expected = if value >= 5 { (480, 640) } else { (640, 480) };
                assert_eq!(
                    sniff(&photo(&exif(&oriented(order, value)))),
                    found(JPEG, expected.0, expected.1),
                    "{order:?} {value}"
                );
            }
        }
        // The Orientation the only entry, and the last one read.
        assert_eq!(
            sniff(&photo(&exif(&tiff(b"MM", &[(0x0112, 3, 1, 6)])))),
            found(JPEG, 480, 640)
        );
        let filler = (0x0100, 4, 1, 0);
        let mut entries = vec![filler; EXIF_ENTRIES_MAX - 1];
        entries.push((0x0112, 3, 1, 6));
        assert_eq!(
            sniff(&photo(&exif(&tiff(b"II", &entries)))),
            found(JPEG, 480, 640)
        );
    }

    #[test]
    fn a_jpeg_with_no_exif_or_a_broken_one_is_sized_as_stored() {
        let stored = found(JPEG, 640, 480);
        let good = oriented(b"II", 6);
        let patched = |tiff: &[u8], at: usize, bytes: &[u8]| {
            let mut tiff = tiff.to_vec();
            tiff[at..at + bytes.len()].copy_from_slice(bytes);
            tiff
        };
        let mut cases: Vec<(&str, Vec<u8>)> = vec![
            ("no APP1", photo(&[])),
            (
                "an APP1 of XMP",
                photo(&segment(0xe1, b"http://ns.adobe.com/xap/1.0/\0<x/>")),
            ),
            (
                "no Exif\\0\\0",
                photo(&segment(0xe1, &[&b"Exif\0"[..], &good].concat())),
            ),
            ("an empty TIFF header", photo(&exif(&[]))),
            (
                "a byte order of IM",
                photo(&exif(&patched(&good, 0, b"IM"))),
            ),
            (
                "a byte order of mm",
                photo(&exif(&patched(&good, 0, b"mm"))),
            ),
            ("43 for 42", photo(&exif(&patched(&good, 2, &[43, 0])))),
            (
                "42 in the other order",
                photo(&exif(&patched(&good, 2, &[0, 42]))),
            ),
            (
                "an IFD past the segment",
                photo(&exif(&patched(&good, 4, &[0xff; 4]))),
            ),
            (
                "an IFD at the segment's last byte",
                photo(&exif(&patched(
                    &good,
                    4,
                    &u32::try_from(good.len() - 1).unwrap().to_le_bytes(),
                ))),
            ),
            // The count says many more entries than the segment holds.
            (
                "an entry count of 65535",
                photo(&exif(&patched(
                    &tiff(b"II", &[(0x010f, 2, 4, 0)]),
                    8,
                    &[0xff; 2],
                ))),
            ),
            ("a LONG", photo(&exif(&tiff(b"II", &[(0x0112, 4, 1, 6)])))),
            (
                "two SHORTs",
                photo(&exif(&tiff(b"MM", &[(0x0112, 3, 2, 6)]))),
            ),
            (
                "no SHORTs",
                photo(&exif(&tiff(b"MM", &[(0x0112, 3, 0, 6)]))),
            ),
            ("an Orientation of 0", photo(&exif(&oriented(b"II", 0)))),
            ("an Orientation of 9", photo(&exif(&oriented(b"MM", 9)))),
            (
                "an Orientation of 65535",
                photo(&exif(&oriented(b"II", 65535))),
            ),
            (
                "an Orientation of 256 + 6",
                photo(&exif(&oriented(b"MM", 0x0106))),
            ),
            (
                "EXIF in APP2",
                photo(&segment(0xe2, &[&b"Exif\0\0"[..], &good].concat())),
            ),
        ];
        // Each TIFF header cut short: within the header, the count, an
        // entry before the Orientation and the Orientation's.
        for length in [0, 1, 2, 4, 7, 8, 9, 10, 15, 21, 22, 23, 29, 31] {
            cases.push(("a cut TIFF header", photo(&exif(&good[..length]))));
        }
        // The Orientation one past the entries read.
        let mut entries = vec![(0x0100, 4, 1, 0); EXIF_ENTRIES_MAX];
        entries.push((0x0112, 3, 1, 6));
        cases.push((
            "an Orientation past the cap",
            photo(&exif(&tiff(b"II", &entries))),
        ));
        for (what, file) in cases {
            assert_eq!(sniff(&file), stored, "{what}");
        }
    }

    #[test]
    fn only_the_first_exif_block_before_the_frame_header_counts() {
        let app = |value| exif(&oriented(b"II", value));
        let two = |first: &[u8], second: &[u8]| jpeg_with(&[first, second], 640, 480).0;
        assert_eq!(sniff(&two(&app(1), &app(6))), found(JPEG, 640, 480));
        assert_eq!(sniff(&two(&app(6), &app(1))), found(JPEG, 480, 640));
        // A broken first block is the one that counts.
        assert_eq!(
            sniff(&two(&exif(&oriented(b"II", 6)[..12]), &app(6))),
            found(JPEG, 640, 480)
        );
        // An APP1 that is not EXIF does not count.
        let xmp = segment(0xe1, b"http://ns.adobe.com/xap/1.0/\0<x/>");
        assert_eq!(sniff(&two(&xmp, &app(6))), found(JPEG, 480, 640));
        // EXIF after the frame header is not read.
        let (mut file, _) = jpeg_with(&[], 640, 480);
        file.truncate(file.len() - 10);
        file.extend(app(6));
        assert_eq!(sniff(&file), found(JPEG, 640, 480));
    }

    #[test]
    fn a_gif_is_its_screen_grown_to_its_first_frame() {
        let canvas = |file: (Vec<u8>, usize)| sniff(&file.0);
        assert_eq!(
            canvas(gif_with(b"GIF89a", (10, 10, 0), &[], (2, 2, 3, 3))),
            found(GIF, 10, 10)
        );
        assert_eq!(
            canvas(gif_with(
                b"GIF89a",
                (1, 1, 0),
                &[],
                (65535, 65535, 65535, 65535)
            )),
            found(GIF, 131_070, 131_070)
        );
        // A colour table of 256 colours.
        let table = [0x2c; 768];
        assert_eq!(
            canvas(gif_with(b"GIF89a", (5, 5, 0x87), &table, (0, 0, 9, 9))),
            found(GIF, 9, 9)
        );
        // A table's size is read only when the flags say there is one.
        assert_eq!(
            canvas(gif_with(b"GIF89a", (5, 5, 0x07), &[], (0, 0, 9, 9))),
            found(GIF, 9, 9)
        );
        // The logical screen's zero side is refused whatever the frame.
        assert_eq!(
            canvas(gif_with(b"GIF89a", (0, 5, 0), &[], (0, 0, 9, 9))),
            bad(GIF, "its width or height is zero")
        );
        // No frame within the bytes: a trailer, an unknown block, or an
        // extension the bytes end in.
        assert_eq!(sniff(&gif_file(b"GIF89a", 6, 7)), found(GIF, 6, 7));
        let mut unknown = gif_with(b"GIF89a", (6, 7, 0), &[], (0, 0, 9, 9)).0;
        unknown[13] = 0x2d;
        assert_eq!(sniff(&unknown), found(GIF, 6, 7));
        let endless = [&[0x21, 0xfe][..], &[1, b'x'].repeat(100)].concat();
        let (file, _) = gif_with(b"GIF89a", (6, 7, 0), &endless, (0, 0, 9, 9));
        assert_eq!(sniff(&file[..file.len() - 20]), found(GIF, 6, 7));
    }

    #[test]
    fn a_gif_is_read_no_further_than_its_cap() {
        // An empty comment is two blocks: its introducer and its end.
        let comments = |n: usize| [0x21, 0xfe, 0].repeat(n);
        let last = gif_with(
            b"GIF89a",
            (1, 1, 0),
            &comments(GIF_BLOCKS_MAX / 2 - 1),
            (0, 0, 9, 9),
        );
        assert_eq!(sniff(&last.0), found(GIF, 9, 9));
        let past = gif_with(
            b"GIF89a",
            (1, 1, 0),
            &comments(GIF_BLOCKS_MAX / 2),
            (0, 0, 9, 9),
        );
        assert_eq!(sniff(&past.0), found(GIF, 1, 1));
    }

    #[test]
    fn an_error_reads_as_a_sentence() {
        assert_eq!(
            SniffError::Unrecognized.to_string(),
            "not a PNG, JPEG, WebP or GIF image"
        );
        assert_eq!(truncated(JPEG).to_string(), "a truncated image/jpeg file");
        assert_eq!(
            invalid(GIF, "its width or height is zero").to_string(),
            "not a valid image/gif file: its width or height is zero"
        );
    }
}
