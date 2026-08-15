//! Which way up a photo actually is.
//!
//! Phones almost never rotate pixels; they store the sensor's native landscape
//! frame and record the intended rotation as metadata. Two different formats,
//! two different places to look:
//!
//! * **HEIC** puts it in an `irot`/`imir` item property (see [`crate::heif`]).
//! * **JPEG** puts it in EXIF tag 0x0112 in an APP1 segment (below).
//!
//! Neither is applied for us: ffmpeg auto-rotates *video* from its display
//! matrix, but not stills, and the HEIC path stitches tiles by hand which would
//! bypass auto-rotation anyway. So both are read here and turned into the filter
//! chain that puts the picture right way up.

use std::path::Path;

/// How a stored image must be transformed before it is displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Orientation {
    /// Anti-clockwise rotation in degrees (0, 90, 180 or 270), as `irot` records it.
    pub rotation_ccw: u16,
    /// Mirroring, applied before the rotation.
    pub mirror: Option<Mirror>,
}

/// The axis an image is mirrored about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mirror {
    /// Top and bottom are swapped.
    Vertical,
    /// Left and right are swapped.
    Horizontal,
}

impl Orientation {
    /// True when the image is stored exactly as it should be shown.
    pub fn is_identity(&self) -> bool {
        self.rotation_ccw == 0 && self.mirror.is_none()
    }

    /// Builds the ffmpeg filter chain that puts the image the right way up, or
    /// None when no transform is needed.
    ///
    /// `irot` counts anti-clockwise while ffmpeg's `transpose` counts clockwise,
    /// so the two quarter-turn cases are swapped relative to what the box says.
    pub fn ffmpeg_filters(&self) -> Option<String> {
        if self.is_identity() {
            return None;
        }
        let mut parts: Vec<&str> = Vec::new();
        match self.mirror {
            Some(Mirror::Horizontal) => parts.push("hflip"),
            Some(Mirror::Vertical) => parts.push("vflip"),
            None => {}
        }
        match self.rotation_ccw {
            90 => parts.push("transpose=2"),
            180 => {
                parts.push("hflip");
                parts.push("vflip");
            }
            270 => parts.push("transpose=1"),
            _ => {}
        }
        (!parts.is_empty()).then(|| parts.join(","))
    }
}

/// Reads a still image's display orientation, choosing the reader by what the
/// file actually is rather than by its extension. Anything unrecognised is
/// treated as already upright.
/// @param path - the image file to inspect
pub fn read(path: &Path) -> Orientation {
    let Some(head) = read_head(path, 1024 * 1024) else {
        return Orientation::default();
    };
    if head.starts_with(&[0xff, 0xd8, 0xff]) {
        return read_jpeg_exif(&head).unwrap_or_default();
    }
    // ISOBMFF (HEIC/HEIF/AVIF): a `ftyp` box at offset 4.
    if head.len() > 8 && &head[4..8] == b"ftyp" {
        return crate::heif::parse_orientation(&head).unwrap_or_default();
    }
    Orientation::default()
}

/// Reads up to `limit` bytes from the start of a file.
/// @param path - file to read
/// @param limit - maximum number of bytes
pub(crate) fn read_head(path: &Path, limit: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut buf = Vec::new();
    file.take(limit as u64).read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Finds the EXIF orientation in a JPEG's APP1 segment and maps it to a
/// rotation/mirror pair. Returns None when the file carries no EXIF orientation.
/// @param buf - the head of the JPEG, covering its marker segments
fn read_jpeg_exif(buf: &[u8]) -> Option<Orientation> {
    let tiff = find_exif_tiff_header(buf)?;
    let value = read_tiff_orientation(buf, tiff)?;
    Some(from_exif_value(value))
}

/// Walks the JPEG marker segments to the start of the TIFF header inside APP1.
fn find_exif_tiff_header(buf: &[u8]) -> Option<usize> {
    let mut pos = 2usize; // skip SOI
    loop {
        // Segments are 0xFF <marker> <2-byte length, inclusive of itself>.
        if *buf.get(pos)? != 0xff {
            return None;
        }
        let marker = *buf.get(pos + 1)?;
        // Start of scan: image data follows, so there is no EXIF after this.
        if marker == 0xda {
            return None;
        }
        let length = u16::from_be_bytes(buf.get(pos + 2..pos + 4)?.try_into().ok()?) as usize;
        if length < 2 {
            return None;
        }
        if marker == 0xe1 && buf.get(pos + 4..pos + 10)? == b"Exif\0\0" {
            return Some(pos + 10);
        }
        pos = pos.checked_add(2)?.checked_add(length)?;
    }
}

/// Reads tag 0x0112 (orientation) from IFD0 of the TIFF block at `tiff`.
fn read_tiff_orientation(buf: &[u8], tiff: usize) -> Option<u16> {
    let byte_order = buf.get(tiff..tiff + 2)?;
    let big_endian = match byte_order {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let raw: [u8; 2] = buf.get(at..at + 2)?.try_into().ok()?;
        Some(if big_endian { u16::from_be_bytes(raw) } else { u16::from_le_bytes(raw) })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let raw: [u8; 4] = buf.get(at..at + 4)?.try_into().ok()?;
        Some(if big_endian { u32::from_be_bytes(raw) } else { u32::from_le_bytes(raw) })
    };

    let ifd0 = tiff.checked_add(u32_at(tiff + 4)? as usize)?;
    let entries = u16_at(ifd0)?;
    for i in 0..entries as usize {
        let entry = ifd0.checked_add(2)?.checked_add(i.checked_mul(12)?)?;
        if u16_at(entry)? == 0x0112 {
            // A SHORT value sits in the first two bytes of the 4-byte value field.
            return u16_at(entry + 8);
        }
    }
    None
}

/// Maps an EXIF orientation code (1-8) onto a rotation/mirror pair.
///
/// EXIF describes the transform needed to display the stored pixels, in
/// clockwise terms; this converts to the anti-clockwise convention used
/// throughout so both formats produce the same filter chain.
fn from_exif_value(value: u16) -> Orientation {
    match value {
        2 => Orientation { rotation_ccw: 0, mirror: Some(Mirror::Horizontal) },
        3 => Orientation { rotation_ccw: 180, mirror: None },
        4 => Orientation { rotation_ccw: 0, mirror: Some(Mirror::Vertical) },
        5 => Orientation { rotation_ccw: 270, mirror: Some(Mirror::Horizontal) },
        6 => Orientation { rotation_ccw: 270, mirror: None },
        7 => Orientation { rotation_ccw: 90, mirror: Some(Mirror::Horizontal) },
        8 => Orientation { rotation_ccw: 90, mirror: None },
        _ => Orientation::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_orientation_needs_no_filter() {
        assert!(Orientation::default().ffmpeg_filters().is_none());
    }

    #[test]
    fn irot_is_translated_from_anticlockwise_to_ffmpeg_clockwise() {
        assert_eq!(
            Orientation { rotation_ccw: 270, mirror: None }.ffmpeg_filters().as_deref(),
            Some("transpose=1")
        );
        assert_eq!(
            Orientation { rotation_ccw: 90, mirror: None }.ffmpeg_filters().as_deref(),
            Some("transpose=2")
        );
    }

    #[test]
    fn mirroring_is_applied_before_the_rotation() {
        let flipped = Orientation { rotation_ccw: 270, mirror: Some(Mirror::Horizontal) };
        assert_eq!(flipped.ffmpeg_filters().as_deref(), Some("hflip,transpose=1"));
    }

    #[test]
    fn exif_six_is_the_common_portrait_case() {
        // EXIF 6 means "rotate 90 clockwise to display", i.e. 270 anti-clockwise.
        assert_eq!(from_exif_value(6), Orientation { rotation_ccw: 270, mirror: None });
        assert_eq!(from_exif_value(1), Orientation::default());
        assert_eq!(from_exif_value(99), Orientation::default());
    }

    #[test]
    fn a_little_endian_jpeg_orientation_is_read_back() {
        // SOI, then an APP1 segment carrying a minimal TIFF block with one IFD0
        // entry: tag 0x0112, type SHORT, count 1, value 6.
        let mut jpeg = vec![0xff, 0xd8];
        let mut tiff: Vec<u8> = Vec::new();
        tiff.extend_from_slice(b"II\x2a\x00");
        tiff.extend_from_slice(&8u32.to_le_bytes()); // IFD0 at offset 8
        tiff.extend_from_slice(&1u16.to_le_bytes()); // one entry
        tiff.extend_from_slice(&0x0112u16.to_le_bytes());
        tiff.extend_from_slice(&3u16.to_le_bytes()); // SHORT
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&6u16.to_le_bytes());
        tiff.extend_from_slice(&[0, 0]);
        let payload_len = (2 + 6 + tiff.len()) as u16;
        jpeg.extend_from_slice(&[0xff, 0xe1]);
        jpeg.extend_from_slice(&payload_len.to_be_bytes());
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(&tiff);
        assert_eq!(read_jpeg_exif(&jpeg), Some(Orientation { rotation_ccw: 270, mirror: None }));
    }

    #[test]
    fn a_jpeg_without_exif_is_treated_as_upright() {
        let jpeg = vec![0xff, 0xd8, 0xff, 0xda, 0x00, 0x02];
        assert_eq!(read_jpeg_exif(&jpeg), None);
    }
}
