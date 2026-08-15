//! Minimal ISOBMFF reader for the one thing an iPhone photo needs and no tool on
//! this box will tell us: which way up it is.
//!
//! A HEIC records its display orientation as an `irot` (rotate) / `imir` (mirror)
//! *item property* on the primary item, not as EXIF and not as stream metadata.
//! `ffprobe` does not surface it, and because the primary image is stored as a
//! tile grid we have to stitch the tiles ourselves (see [`crate::imaging`]) —
//! which bypasses ffmpeg's own auto-rotation. Without this module every portrait
//! photo in the library comes out lying on its side.
//!
//! Only the boxes on the path to that answer are parsed:
//! `meta` -> `pitm` (primary item id) and `iprp` -> { `ipco` (property array),
//! `ipma` (item -> property associations) }. Everything is bounds-checked and
//! returns `None` rather than panicking, since these are files from a phone.

use std::path::Path;

use crate::orientation::{read_head, Mirror, Orientation};

/// Reads the primary item's display orientation from a HEIC/HEIF file.
///
/// Only the head of the file is read: the `meta` box that carries the item
/// properties sits near the front, well before the image data. Returns None for
/// anything that is not a parseable HEIF, so callers can fall back to treating
/// the image as upright.
/// @param path - the .heic/.heif file to inspect
pub fn read_orientation(path: &Path) -> Option<Orientation> {
    // 1 MiB comfortably covers the metadata of the iPhone files in the library
    // (their `meta` boxes end within ~40 KB) without reading a 4 MB photo.
    let head = read_head(path, 1024 * 1024)?;
    parse_orientation(&head)
}

/// A parsed box header: its four-character type and the span of its payload.
struct BoxRef {
    kind: [u8; 4],
    start: usize,
    end: usize,
}

impl BoxRef {
    /// True if this box has the given four-character type.
    fn is(&self, kind: &[u8; 4]) -> bool {
        &self.kind == kind
    }
}

/// Reads the box header at `pos`, returning it and the offset of the next
/// sibling. Handles both 32-bit sizes and the 64-bit `largesize` escape.
fn read_box(buf: &[u8], pos: usize, limit: usize) -> Option<(BoxRef, usize)> {
    if pos.checked_add(8)? > limit {
        return None;
    }
    let mut size = u32::from_be_bytes(buf.get(pos..pos + 4)?.try_into().ok()?) as usize;
    let kind: [u8; 4] = buf.get(pos + 4..pos + 8)?.try_into().ok()?;
    let mut header = 8usize;
    if size == 1 {
        if pos.checked_add(16)? > limit {
            return None;
        }
        size = u64::from_be_bytes(buf.get(pos + 8..pos + 16)?.try_into().ok()?) as usize;
        header = 16;
    } else if size == 0 {
        size = limit.checked_sub(pos)?;
    }
    if size < header || pos.checked_add(size)? > limit {
        return None;
    }
    Some((BoxRef { kind, start: pos + header, end: pos + size }, pos + size))
}

/// Finds a direct child box of the given type within `[start, end)`.
fn find_box(buf: &[u8], start: usize, end: usize, kind: &[u8; 4]) -> Option<BoxRef> {
    let mut pos = start;
    while pos < end {
        let (found, next) = read_box(buf, pos, end)?;
        if found.is(kind) {
            return Some(found);
        }
        pos = next;
    }
    None
}

/// Collects every direct child box within `[start, end)`, in file order — the
/// order `ipma` indexes into.
fn list_boxes(buf: &[u8], start: usize, end: usize) -> Vec<BoxRef> {
    let mut out = Vec::new();
    let mut pos = start;
    while pos < end {
        match read_box(buf, pos, end) {
            Some((found, next)) => {
                out.push(found);
                pos = next;
            }
            None => break,
        }
    }
    out
}

/// Walks `meta` -> `pitm`/`iprp` to resolve the primary item's orientation.
/// Split out from [`read_orientation`] so it can be unit-tested on bytes.
/// @param buf - the head of the file, covering at least the `meta` box
pub(crate) fn parse_orientation(buf: &[u8]) -> Option<Orientation> {
    let meta = find_box(buf, 0, buf.len(), b"meta")?;
    // `meta` is a FullBox: four bytes of version/flags precede its children.
    let meta_children = meta.start.checked_add(4)?;
    if meta_children > meta.end {
        return None;
    }

    let primary_id = read_primary_item_id(buf, meta_children, meta.end)?;
    let iprp = find_box(buf, meta_children, meta.end, b"iprp")?;
    let ipco = find_box(buf, iprp.start, iprp.end, b"ipco")?;
    let ipma = find_box(buf, iprp.start, iprp.end, b"ipma")?;
    let properties = list_boxes(buf, ipco.start, ipco.end);
    let indices = property_indices_for(buf, &ipma, primary_id)?;

    let mut orientation = Orientation::default();
    for index in indices {
        // `ipma` indices are 1-based into the `ipco` array.
        let Some(property) = index.checked_sub(1).and_then(|i| properties.get(i as usize)) else {
            continue;
        };
        let Some(&payload) = buf.get(property.start) else {
            continue;
        };
        if property.is(b"irot") {
            orientation.rotation_ccw = (payload as u16 & 0b11) * 90;
        } else if property.is(b"imir") {
            orientation.mirror = Some(if payload & 1 == 1 { Mirror::Vertical } else { Mirror::Horizontal });
        }
    }
    Some(orientation)
}

/// Reads the primary item id from the `pitm` box.
fn read_primary_item_id(buf: &[u8], start: usize, end: usize) -> Option<u32> {
    let pitm = find_box(buf, start, end, b"pitm")?;
    let version = *buf.get(pitm.start)?;
    let id_at = pitm.start.checked_add(4)?;
    if version == 0 {
        Some(u16::from_be_bytes(buf.get(id_at..id_at + 2)?.try_into().ok()?) as u32)
    } else {
        Some(u32::from_be_bytes(buf.get(id_at..id_at + 4)?.try_into().ok()?))
    }
}

/// Returns the property indices `ipma` associates with `item_id`.
///
/// Entry layout is version/flags-dependent: item ids widen to 32 bits at
/// version 1, and property indices widen to 15 bits when flag bit 0 is set.
fn property_indices_for(buf: &[u8], ipma: &BoxRef, item_id: u32) -> Option<Vec<u16>> {
    let header = u32::from_be_bytes(buf.get(ipma.start..ipma.start + 4)?.try_into().ok()?);
    let version = header >> 24;
    let wide_indices = header & 1 == 1;

    let mut pos = ipma.start.checked_add(4)?;
    let entry_count = u32::from_be_bytes(buf.get(pos..pos + 4)?.try_into().ok()?);
    pos = pos.checked_add(4)?;

    for _ in 0..entry_count {
        let (id, id_width) = if version < 1 {
            (u16::from_be_bytes(buf.get(pos..pos + 2)?.try_into().ok()?) as u32, 2)
        } else {
            (u32::from_be_bytes(buf.get(pos..pos + 4)?.try_into().ok()?), 4)
        };
        pos = pos.checked_add(id_width)?;
        let count = *buf.get(pos)? as usize;
        pos = pos.checked_add(1)?;

        let mut indices = Vec::with_capacity(count);
        for _ in 0..count {
            if wide_indices {
                indices.push(u16::from_be_bytes(buf.get(pos..pos + 2)?.try_into().ok()?) & 0x7fff);
                pos = pos.checked_add(2)?;
            } else {
                indices.push((*buf.get(pos)? & 0x7f) as u16);
                pos = pos.checked_add(1)?;
            }
        }
        if id == item_id {
            return Some(indices);
        }
    }
    // A file with no association for its primary item is upright by default.
    Some(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wraps a payload in a box header of the given four-character type.
    fn boxed(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    /// Builds a minimal HEIF `meta` box whose primary item (id 1) is associated
    /// with an `irot` of the given quarter-turn count, plus a decoy property so
    /// the 1-based `ipma` indexing is actually exercised.
    fn meta_with_irot(quarter_turns: u8) -> Vec<u8> {
        let pitm = boxed(b"pitm", &[0, 0, 0, 0, 0, 1]);
        let mut ipco_body = boxed(b"ispe", &[0; 12]);
        ipco_body.extend_from_slice(&boxed(b"irot", &[quarter_turns]));
        let ipco = boxed(b"ipco", &ipco_body);
        // version 0, flags 0, one entry: item 1, two properties (ispe=1, irot=2).
        let ipma = boxed(b"ipma", &[0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 2, 1, 2]);
        let mut iprp_body = ipco;
        iprp_body.extend_from_slice(&ipma);
        let iprp = boxed(b"iprp", &iprp_body);

        let mut meta_body = vec![0, 0, 0, 0]; // FullBox version/flags
        meta_body.extend_from_slice(&pitm);
        meta_body.extend_from_slice(&iprp);
        boxed(b"meta", &meta_body)
    }

    #[test]
    fn the_primary_items_irot_is_read_through_ipma() {
        assert_eq!(
            parse_orientation(&meta_with_irot(3)),
            Some(Orientation { rotation_ccw: 270, mirror: None })
        );
        assert_eq!(parse_orientation(&meta_with_irot(0)), Some(Orientation::default()));
        assert_eq!(
            parse_orientation(&meta_with_irot(2)),
            Some(Orientation { rotation_ccw: 180, mirror: None })
        );
    }

    #[test]
    fn a_file_with_no_meta_box_has_no_answer() {
        assert!(parse_orientation(&boxed(b"ftyp", b"heic")).is_none());
    }

    #[test]
    fn garbage_is_rejected_rather_than_panicking() {
        assert!(parse_orientation(&[]).is_none());
        assert!(parse_orientation(&[0, 0, 0, 8, b'm', b'e', b't', b'a']).is_none());
        // A truncated `meta` box must not read past the buffer.
        assert!(parse_orientation(&[0, 0, 0xff, 0xff, b'm', b'e', b't', b'a', 0, 0, 0, 0]).is_none());
    }
}
