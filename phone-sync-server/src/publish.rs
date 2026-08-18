//! Publishing items from the private library to the public media site (task-1569).
//!
//! Phone Sync is a private backup. `media.jasonmcaffee.com` is a public gallery.
//! This module is the door between them, and it is built so the door only opens
//! outward:
//!
//! * Publishing renders **derivatives** into `<data dir>/published/<public id>/`.
//!   The originals under the media root are only ever read — never moved,
//!   modified or deleted.
//! * Every published item gets a **fresh random public id**. The public surface
//!   never exposes and never accepts a sha256, so it cannot be used as an oracle
//!   for "does this exact file exist in Jason's library".
//! * A public asset request resolves `public id + variant` to a fixed file name
//!   inside one directory. No caller-supplied path fragment ever reaches the
//!   filesystem, and the resolved path is checked to be inside the publish root.
//! * An unknown or unpublished id is a uniform 404 either way.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use anyhow::{Context, Result};
use image::GenericImageView;
use serde::{Deserialize, Serialize};

use crate::imaging::{self, MediaTools};
use crate::models::MediaRecord;
use crate::storage::Storage;
use crate::transcode::{self, RENDITIONS};

/// Longest edge of the tile rendition shown in the stream.
const GRID_MAX_DIM: u32 = 720;
/// Longest edge of the full rendition shown in the detail view.
const FULL_MAX_DIM: u32 = 2560;
/// Width of the inline placeholder encoded into the feed.
const LQIP_WIDTH: u32 = 24;

/// One published item, as persisted in `published/index.json`.
///
/// `sha256` is the link back to the private record. It is serialized to disk so
/// the gallery can show what is already published, and it is **never** part of a
/// public response — that is enforced by [`PublicItem`] being a separate struct
/// rather than by a skip attribute that a later edit could quietly drop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishedItem {
    pub public_id: String,
    pub sha256: String,
    /// "photo" or "video".
    pub kind: String,
    /// RFC-3339 capture time carried over from the source record.
    pub captured_at: String,
    /// Unix seconds when this item was published.
    pub published_at: i64,
    /// Original filename, kept for the detail view's edge print.
    pub filename: String,
    /// Dimensions of the `full` rendition. The stream needs true aspect ratios
    /// to lay out justified rows before any image has loaded.
    pub width: u32,
    pub height: u32,
    /// Playing time, videos only.
    pub duration_secs: Option<f64>,
    pub title: Option<String>,
    pub caption: Option<String>,
    /// Whether this item appears in the Theatre section.
    pub featured: bool,
    /// Dominant colour, computed at publish time. The site's WebGL light field
    /// lerps toward the colours of whatever is currently in view, and doing it
    /// here means the client never has to sample pixels off a canvas.
    pub color: [u8; 3],
    /// A ~24px-wide JPEG as a data URI, under a kilobyte, shown while the real
    /// tile loads.
    pub lqip: String,
    /// Byte size of each variant that exists, keyed by variant name.
    pub bytes: BTreeMap<String, u64>,
}

/// The same item as the public feed sees it: everything except the content hash.
#[derive(Debug, Clone, Serialize)]
pub struct PublicItem {
    pub public_id: String,
    pub kind: String,
    pub captured_at: String,
    pub published_at: i64,
    pub width: u32,
    pub height: u32,
    pub duration_secs: Option<f64>,
    pub title: Option<String>,
    pub caption: Option<String>,
    pub featured: bool,
    pub color: [u8; 3],
    pub lqip: String,
    pub bytes: BTreeMap<String, u64>,
    /// Which variants actually exist for this item, so the client never requests
    /// a rendition that was skipped.
    pub variants: Vec<String>,
}

impl From<&PublishedItem> for PublicItem {
    fn from(item: &PublishedItem) -> Self {
        PublicItem {
            public_id: item.public_id.clone(),
            kind: item.kind.clone(),
            captured_at: item.captured_at.clone(),
            published_at: item.published_at,
            width: item.width,
            height: item.height,
            duration_secs: item.duration_secs,
            title: item.title.clone(),
            caption: item.caption.clone(),
            featured: item.featured,
            color: item.color,
            lqip: item.lqip.clone(),
            bytes: item.bytes.clone(),
            variants: item.bytes.keys().cloned().collect(),
        }
    }
}

/// The renditions a published item can have. Parsing is an exhaustive match on a
/// closed set, so the file name a request resolves to is always one of ours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// The stream tile — a JPEG for photos and videos alike.
    Grid,
    /// The detail view image, or a video's full-size poster frame.
    Full,
    /// A three second silent clip the grid tile plays on hover.
    Loop,
    /// 540p H.264, the fallback rendition.
    V540,
    /// 1080p H.264, the primary rendition.
    V1080,
}

impl Variant {
    /// Resolves a URL segment to a variant, or None for anything unrecognised.
    ///
    /// The segment may carry the rendition's own file extension (`grid.jpg`,
    /// `v1080.mp4`). That is not cosmetic: Cloudflare's default cache level keys
    /// off the file extension, so extension-less asset URLs came back
    /// `cf-cache-status: DYNAMIC` and every photograph round-tripped to this box
    /// for every visitor. Both spellings are accepted, so a link that predates
    /// the change keeps resolving.
    /// @param raw - the `:variant` path segment as received
    pub fn parse(raw: &str) -> Option<Variant> {
        let stem = raw.split('.').next().unwrap_or(raw);
        let variant = match stem {
            "grid" => Variant::Grid,
            "full" => Variant::Full,
            "loop" => Variant::Loop,
            "v540" => Variant::V540,
            "v1080" => Variant::V1080,
            _ => return None,
        };
        // An extension that contradicts the rendition is not one of ours.
        match raw.split_once('.') {
            None => Some(variant),
            Some((_, ext)) if variant.extension() == ext => Some(variant),
            Some(_) => None,
        }
    }

    /// The file extension this rendition is served under.
    pub fn extension(self) -> &'static str {
        match self {
            Variant::Grid | Variant::Full => "jpg",
            Variant::Loop | Variant::V540 | Variant::V1080 => "mp4",
        }
    }

    /// The fixed file name this variant lives under inside an item's directory.
    pub fn file_name(self) -> &'static str {
        match self {
            Variant::Grid => "grid.jpg",
            Variant::Full => "full.jpg",
            Variant::Loop => "loop.mp4",
            Variant::V540 => "v540.mp4",
            Variant::V1080 => "v1080.mp4",
        }
    }

    /// The name this variant is keyed by in `bytes` / `variants`.
    pub fn key(self) -> &'static str {
        match self {
            Variant::Grid => "grid",
            Variant::Full => "full",
            Variant::Loop => "loop",
            Variant::V540 => "v540",
            Variant::V1080 => "v1080",
        }
    }

    /// The MIME type this variant is served as.
    pub fn content_type(self) -> &'static str {
        match self {
            Variant::Grid | Variant::Full => "image/jpeg",
            Variant::Loop | Variant::V540 | Variant::V1080 => "video/mp4",
        }
    }
}

/// Fields a caller may set when publishing or editing an item.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct PublishFields {
    pub title: Option<String>,
    pub caption: Option<String>,
    pub featured: Option<bool>,
}

/// The published set plus the directory its derivatives live in.
///
/// The index is small (a curated selection, not the whole library) and is read on
/// every feed request, so it is held in memory behind an `RwLock` and written
/// through to disk on every mutation.
pub struct PublishStore {
    root: PathBuf,
    items: RwLock<Vec<PublishedItem>>,
}

impl PublishStore {
    /// Opens (and creates on first run) the publish root, loading the index.
    /// @param data_dir - the server's data directory
    pub fn open(data_dir: &Path) -> Result<Self> {
        let root = data_dir.join("published");
        std::fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
        let index_path = root.join("index.json");
        let mut items: Vec<PublishedItem> = match std::fs::read(&index_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::error!("published index is unreadable ({e}); starting from an empty set");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        sort_newest_first(&mut items);
        Ok(PublishStore { root, items: RwLock::new(items) })
    }

    /// Every published item, newest capture first.
    pub fn all(&self) -> Vec<PublishedItem> {
        self.items.read().expect("publish index lock").clone()
    }

    /// One page of the public feed, optionally filtered by kind.
    /// @param offset - how many items to skip
    /// @param limit - how many to return
    /// @param kind - "photo", "video", "featured", or None for everything
    pub fn page(&self, offset: usize, limit: usize, kind: Option<&str>) -> (Vec<PublicItem>, usize) {
        let items = self.items.read().expect("publish index lock");
        let filtered: Vec<&PublishedItem> = items
            .iter()
            .filter(|item| match kind {
                Some("photo") => item.kind == "photo",
                Some("video") => item.kind == "video",
                Some("featured") => item.featured,
                _ => true,
            })
            .collect();
        let total = filtered.len();
        let page = filtered.into_iter().skip(offset).take(limit).map(PublicItem::from).collect();
        (page, total)
    }

    /// How many photos and videos are published.
    pub fn counts(&self) -> (usize, usize) {
        let items = self.items.read().expect("publish index lock");
        let videos = items.iter().filter(|i| i.kind == "video").count();
        (items.len() - videos, videos)
    }

    /// Looks up a published item by its public id.
    /// @param public_id - the id from a public URL
    pub fn by_public_id(&self, public_id: &str) -> Option<PublishedItem> {
        self.items.read().expect("publish index lock").iter().find(|i| i.public_id == public_id).cloned()
    }

    /// Looks up a published item by the private content hash it came from.
    /// @param sha256 - the source record's content hash
    pub fn by_sha256(&self, sha256: &str) -> Option<PublishedItem> {
        self.items.read().expect("publish index lock").iter().find(|i| i.sha256 == sha256).cloned()
    }

    /// Resolves a public id and variant to a file on disk, refusing anything that
    /// does not land inside this item's own directory.
    /// @param public_id - the id from a public URL
    /// @param variant - the parsed variant
    pub fn variant_path(&self, public_id: &str, variant: Variant) -> Option<PathBuf> {
        // The id came off a URL, so it is treated as hostile until proven to be
        // one of ours: it must be in the index, and the index only ever holds ids
        // this module generated.
        let item = self.by_public_id(public_id)?;
        if !item.bytes.contains_key(variant.key()) {
            return None;
        }
        let path = self.root.join(&item.public_id).join(variant.file_name());
        // Belt and braces: even with a known-good id and a fixed file name, the
        // resolved path is confirmed to be under the publish root before it is
        // opened.
        let canonical_root = self.root.canonicalize().ok()?;
        let canonical = path.canonicalize().ok()?;
        canonical.starts_with(&canonical_root).then_some(canonical)
    }

    /// Edits the caption fields of a published item.
    /// @param public_id - which item to edit
    /// @param fields - the values to apply; None leaves a field unchanged
    pub fn update(&self, public_id: &str, fields: &PublishFields) -> Result<Option<PublishedItem>> {
        let updated = {
            let mut items = self.items.write().expect("publish index lock");
            let Some(item) = items.iter_mut().find(|i| i.public_id == public_id) else {
                return Ok(None);
            };
            if let Some(title) = &fields.title {
                item.title = non_empty(title);
            }
            if let Some(caption) = &fields.caption {
                item.caption = non_empty(caption);
            }
            if let Some(featured) = fields.featured {
                item.featured = featured;
            }
            item.clone()
        };
        self.persist()?;
        Ok(Some(updated))
    }

    /// Unpublishes an item and deletes the derivatives this module produced for
    /// it. Nothing under the media root is touched — the original photo or video
    /// is untouched and still in the private library.
    /// @param public_id - which item to unpublish
    pub fn remove(&self, public_id: &str) -> Result<bool> {
        let removed = {
            let mut items = self.items.write().expect("publish index lock");
            let before = items.len();
            items.retain(|i| i.public_id != public_id);
            before != items.len()
        };
        if !removed {
            return Ok(false);
        }
        self.persist()?;
        // Only ever a directory this module created, named by an id it generated.
        let dir = self.root.join(public_id);
        if dir.starts_with(&self.root) && dir.is_dir() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        Ok(true)
    }

    /// Renders every derivative for a record and adds it to the published set.
    ///
    /// Re-publishing an item that is already published returns the existing entry
    /// with any supplied fields applied, rather than producing a second copy.
    /// @param storage - the private library, read-only here
    /// @param tools - resolved ffmpeg/ffprobe paths
    /// @param record - the source record to publish
    /// @param fields - optional title/caption/featured
    pub fn publish(&self, storage: &Storage, tools: &MediaTools, record: &MediaRecord, fields: &PublishFields) -> Result<PublishedItem> {
        if let Some(existing) = self.by_sha256(&record.sha256) {
            if let Some(updated) = self.update(&existing.public_id, fields)? {
                return Ok(updated);
            }
            return Ok(existing);
        }

        let public_id = new_public_id();
        let dir = self.root.join(&public_id);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

        let result = self.render_derivatives(storage, tools, record, &public_id, &dir, fields);
        if result.is_err() {
            // A half-rendered item must never enter the index; clean up the
            // directory this call created rather than leaving debris behind.
            let _ = std::fs::remove_dir_all(&dir);
        }
        let item = result?;

        {
            let mut items = self.items.write().expect("publish index lock");
            items.push(item.clone());
            sort_newest_first(&mut items);
        }
        self.persist()?;
        Ok(item)
    }

    /// Produces the JPEG and MP4 renditions for one record and builds its index
    /// entry. Split out so a failure can be cleaned up by the caller.
    /// @param storage - the private library, read-only here
    /// @param tools - resolved ffmpeg/ffprobe paths
    /// @param record - the source record
    /// @param public_id - the id already allocated for it
    /// @param dir - the directory the derivatives are written into
    /// @param fields - optional title/caption/featured
    fn render_derivatives(&self, storage: &Storage, tools: &MediaTools, record: &MediaRecord, public_id: &str, dir: &Path, fields: &PublishFields) -> Result<PublishedItem> {
        let source = storage.absolute_path(record);
        anyhow::ensure!(source.exists(), "source file is missing: {}", source.display());
        let is_video = record.media_type == "video";

        // The still renderer already handles the iPhone HEIC tile grid, the
        // gain-map trap and `irot` orientation, and ffmpeg applies a video's own
        // rotation matrix, so both kinds arrive here correctly oriented.
        let full_jpeg = if is_video {
            imaging::render_video_frame(tools, &source, FULL_MAX_DIM)
        } else {
            imaging::render_still(tools, &source, FULL_MAX_DIM)
        }
        .ok_or_else(|| anyhow::anyhow!("could not render an image for {}", record.filename))?;

        let grid_jpeg = if is_video {
            imaging::render_video_frame(tools, &source, GRID_MAX_DIM)
        } else {
            imaging::render_still(tools, &source, GRID_MAX_DIM)
        }
        .ok_or_else(|| anyhow::anyhow!("could not render a tile for {}", record.filename))?;

        let mut bytes = BTreeMap::new();
        std::fs::write(dir.join(Variant::Full.file_name()), &full_jpeg)?;
        bytes.insert(Variant::Full.key().to_string(), full_jpeg.len() as u64);
        std::fs::write(dir.join(Variant::Grid.file_name()), &grid_jpeg)?;
        bytes.insert(Variant::Grid.key().to_string(), grid_jpeg.len() as u64);

        // Dimensions come from the rendition that was actually produced, not from
        // the source's metadata: it is the one that has been rotated, stitched and
        // scaled, so it is the only thing whose aspect ratio the layout can trust.
        let decoded = image::load_from_memory(&full_jpeg).context("decoding the rendered full image")?;
        let (width, height) = decoded.dimensions();
        let color = dominant_color(&decoded);
        let lqip = build_lqip(&decoded)?;

        let mut duration_secs = None;
        if is_video {
            let info = transcode::probe_video(tools, &source).unwrap_or_default();
            duration_secs = (info.duration_secs > 0.0).then_some(info.duration_secs);

            for rendition in RENDITIONS.iter() {
                if !transcode::is_worth_producing(rendition, &info) {
                    continue;
                }
                let dest = dir.join(rendition.file_name);
                transcode::transcode_rendition(tools, &source, &dest, rendition, &info)
                    .map_err(|e| anyhow::anyhow!("transcoding {} to {}: {e}", record.filename, rendition.file_name))?;
                let key = rendition.file_name.trim_end_matches(".mp4").to_string();
                bytes.insert(key, std::fs::metadata(&dest)?.len());
            }

            // The hover loop is a nicety, not a requirement — a clip too short or
            // too odd to loop simply does not get one, and the tile falls back to
            // its poster.
            let loop_dest = dir.join(Variant::Loop.file_name());
            match transcode::render_loop_clip(tools, &source, &loop_dest, &info) {
                Ok(()) => {
                    bytes.insert(Variant::Loop.key().to_string(), std::fs::metadata(&loop_dest)?.len());
                }
                Err(e) => tracing::warn!("no hover loop for {}: {e}", record.filename),
            }

            anyhow::ensure!(
                bytes.contains_key("v540") || bytes.contains_key("v1080"),
                "no playable rendition was produced for {}",
                record.filename
            );
        }

        Ok(PublishedItem {
            public_id: public_id.to_string(),
            sha256: record.sha256.clone(),
            kind: if is_video { "video".into() } else { "photo".into() },
            captured_at: record.created_at.clone(),
            published_at: chrono::Utc::now().timestamp(),
            filename: record.filename.clone(),
            width,
            height,
            duration_secs,
            title: fields.title.as_deref().and_then(non_empty),
            caption: fields.caption.as_deref().and_then(non_empty),
            featured: fields.featured.unwrap_or(false),
            color,
            lqip,
            bytes,
        })
    }

    /// Writes the index to disk, via a temp file and a rename so a crash midway
    /// cannot leave a truncated index behind.
    fn persist(&self) -> Result<()> {
        let items = self.items.read().expect("publish index lock");
        let json = serde_json::to_vec_pretty(&*items)?;
        let tmp = self.root.join("index.json.tmp");
        std::fs::write(&tmp, &json)?;
        std::fs::rename(&tmp, self.root.join("index.json"))?;
        Ok(())
    }
}

/// Orders the index newest capture first, which is the order the feed serves.
/// @param items - the index to sort in place
fn sort_newest_first(items: &mut [PublishedItem]) {
    items.sort_by(|a, b| b.captured_at.cmp(&a.captured_at).then_with(|| b.published_at.cmp(&a.published_at)));
}

/// Trims a string and returns None when nothing is left.
/// @param value - the raw field value
fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Generates the random public id an item is addressed by on the public site.
///
/// Deliberately not derived from the content hash: a public id that could be
/// computed from a file would let anyone test whether a given photo is in this
/// library.
fn new_public_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..16].to_string()
}

/// Computes the colour the site's light field borrows from this photograph.
///
/// A plain average is usually muddy — every photograph averages to a grey-brown —
/// so the mean is pushed away from its own luminance to recover the saturation
/// the averaging removed. The result is the colour the picture *feels* like
/// rather than the colour it literally means.
/// @param image - the decoded full rendition
fn dominant_color(image: &image::DynamicImage) -> [u8; 3] {
    let small = image.resize_exact(16, 16, image::imageops::FilterType::Triangle).to_rgb8();
    let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
    for pixel in small.pixels() {
        r += pixel[0] as u64;
        g += pixel[1] as u64;
        b += pixel[2] as u64;
    }
    let count = (small.width() * small.height()) as u64;
    let mean = [(r / count) as f32, (g / count) as f32, (b / count) as f32];
    let luma = 0.2126 * mean[0] + 0.7152 * mean[1] + 0.0722 * mean[2];
    let boosted = mean.map(|channel| (luma + (channel - luma) * 1.6).clamp(0.0, 255.0));
    [boosted[0] as u8, boosted[1] as u8, boosted[2] as u8]
}

/// Builds the inline placeholder: a 24px-wide JPEG as a data URI.
///
/// Small enough to travel in the feed JSON (well under a kilobyte), big enough to
/// carry the photograph's real composition, so a tile is never a grey box.
/// @param image - the decoded full rendition
fn build_lqip(image: &image::DynamicImage) -> Result<String> {
    let (width, height) = image.dimensions();
    let target_height = ((LQIP_WIDTH as f32 / width.max(1) as f32) * height as f32).round().max(1.0) as u32;
    let tiny = image.resize_exact(LQIP_WIDTH, target_height, image::imageops::FilterType::Triangle).to_rgb8();

    let mut jpeg = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 40);
    encoder.encode(tiny.as_raw(), tiny.width(), tiny.height(), image::ExtendedColorType::Rgb8)?;
    Ok(format!("data:image/jpeg;base64,{}", base64_encode(&jpeg)))
}

/// Standard base64, used only to inline the placeholder above.
///
/// Written out rather than pulled in as a dependency: this is the one place in
/// the server that needs base64 at all, and it is fifteen lines.
/// @param input - the bytes to encode
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_test_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn only_the_known_variants_parse() {
        assert_eq!(Variant::parse("grid"), Some(Variant::Grid));
        assert_eq!(Variant::parse("v1080"), Some(Variant::V1080));
        assert_eq!(Variant::parse("../../jwt-secret"), None);
        assert_eq!(Variant::parse("index.json"), None);
        assert_eq!(Variant::parse(""), None);
    }

    #[test]
    fn a_variant_may_carry_its_own_extension() {
        assert_eq!(Variant::parse("grid.jpg"), Some(Variant::Grid));
        assert_eq!(Variant::parse("full.jpg"), Some(Variant::Full));
        assert_eq!(Variant::parse("v540.mp4"), Some(Variant::V540));
        // A contradictory or unknown extension is refused rather than ignored.
        assert_eq!(Variant::parse("grid.mp4"), None);
        assert_eq!(Variant::parse("v1080.exe"), None);
        assert_eq!(Variant::parse("grid.jpg.exe"), None);
    }

    #[test]
    fn a_public_item_cannot_carry_the_content_hash() {
        let item = PublishedItem {
            public_id: "abc123".into(),
            sha256: "deadbeef".into(),
            kind: "photo".into(),
            captured_at: "2026-08-01T00:00:00Z".into(),
            published_at: 0,
            filename: "IMG_0001.HEIC".into(),
            width: 100,
            height: 80,
            duration_secs: None,
            title: None,
            caption: None,
            featured: false,
            color: [1, 2, 3],
            lqip: "data:,".into(),
            bytes: BTreeMap::from([("grid".to_string(), 10)]),
        };
        let json = serde_json::to_string(&PublicItem::from(&item)).unwrap();
        assert!(!json.contains("deadbeef"), "the public feed leaked a content hash: {json}");
        assert!(!json.contains("sha256"));
        assert!(json.contains("\"variants\":[\"grid\"]"));
    }

    #[test]
    fn public_ids_are_unpredictable_and_url_safe() {
        let a = new_public_id();
        let b = new_public_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn the_index_is_ordered_newest_capture_first() {
        let mut items = vec![
            PublishedItem { captured_at: "2026-01-01T00:00:00Z".into(), ..sample("old") },
            PublishedItem { captured_at: "2026-08-01T00:00:00Z".into(), ..sample("new") },
        ];
        sort_newest_first(&mut items);
        assert_eq!(items[0].public_id, "new");
    }

    /// A minimal item for ordering tests.
    fn sample(public_id: &str) -> PublishedItem {
        PublishedItem {
            public_id: public_id.into(),
            sha256: String::new(),
            kind: "photo".into(),
            captured_at: String::new(),
            published_at: 0,
            filename: String::new(),
            width: 1,
            height: 1,
            duration_secs: None,
            title: None,
            caption: None,
            featured: false,
            color: [0, 0, 0],
            lqip: String::new(),
            bytes: BTreeMap::new(),
        }
    }
}
