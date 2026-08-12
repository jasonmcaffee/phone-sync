//! Filesystem-backed media storage with a JSON metadata index.
//!
//! Uploads are filed into a date-organized tree under the configured media root
//! — `<media_root>/<year>/<yyyymm>-<suffix>/<original filename>`, e.g.
//! `E:\pictures\2026\202608-phone-sync\IMG_0001.HEIC` — so the backup lands
//! directly in the same photo library layout everything else on this machine
//! uses, rather than in an opaque hash tree.
//!
//! Content is still de-duplicated by sha256: re-uploading bytes that are already
//! stored reuses the existing file instead of writing a second copy. A JSON
//! index maps asset ids -> records and is persisted atomically.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime};
use sha2::{Digest, Sha256};

use crate::models::{MediaRecord, StorageRoot};

/// Makes each atomic-write temp filename unique within the process.
static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Owns the storage roots and an in-memory index guarded by a mutex.
pub struct Storage {
    /// Holds the metadata index and the thumbnail cache.
    data_dir: PathBuf,
    /// Root of the date-organized photo/video tree.
    media_root: PathBuf,
    /// Suffix appended to each month folder (`202608-phone-sync`).
    folder_suffix: String,
    /// Serializes the whole "is it already stored? / pick a free name / write"
    /// sequence, so two concurrent uploads can neither pick the same filename
    /// nor both write the same new content.
    write_lock: Mutex<()>,
    /// asset_id -> record. Protected for concurrent uploads.
    index: Mutex<HashMap<String, MediaRecord>>,
}

impl Storage {
    /// Opens (or initializes) storage, creating the index/thumbnail dirs and the
    /// media root, and loading any existing metadata index from disk.
    /// @param data_dir - directory holding the index and thumbnail cache
    /// @param media_root - root of the date-organized media tree
    /// @param folder_suffix - suffix appended to each month folder name
    pub fn open(data_dir: PathBuf, media_root: PathBuf, folder_suffix: String) -> Result<Self> {
        std::fs::create_dir_all(&media_root).context("creating media root")?;
        std::fs::create_dir_all(data_dir.join("index")).context("creating index dir")?;
        std::fs::create_dir_all(data_dir.join("thumbs")).context("creating thumbs dir")?;
        let index = load_index(&data_dir).unwrap_or_default();
        Ok(Self {
            data_dir,
            media_root,
            folder_suffix,
            write_lock: Mutex::new(()),
            index: Mutex::new(index),
        })
    }

    /// Returns the set of asset ids already stored, for the client manifest.
    pub fn known_asset_ids(&self) -> Vec<String> {
        self.index.lock().unwrap().keys().cloned().collect()
    }

    /// Fetches a stored record by its sha256 id, if present.
    pub fn get_by_id(&self, id: &str) -> Option<MediaRecord> {
        self.index
            .lock()
            .unwrap()
            .values()
            .find(|r| r.sha256 == id)
            .cloned()
    }

    /// Returns all stored records, newest first (by capture time), for the
    /// web gallery listing.
    pub fn all_records(&self) -> Vec<MediaRecord> {
        let mut records: Vec<MediaRecord> = self.index.lock().unwrap().values().cloned().collect();
        records.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        records
    }

    /// Path of the cached JPEG thumbnail for a content hash.
    fn thumb_path(&self, sha256: &str) -> PathBuf {
        self.data_dir.join("thumbs").join(format!("{}.jpg", sha256))
    }

    /// True if a thumbnail (client-uploaded or previously generated) exists.
    /// @param sha256 - content hash
    pub fn has_thumbnail(&self, sha256: &str) -> bool {
        self.thumb_path(sha256).exists()
    }

    /// Stores a client-provided JPEG thumbnail for a content hash. iOS can
    /// thumbnail HEIC and video (which the server's image decoder can't), so the
    /// app uploads previews here for everything.
    /// @param sha256 - content hash the thumbnail belongs to
    /// @param jpeg - the JPEG thumbnail bytes
    pub fn store_thumbnail(&self, sha256: &str, jpeg: &[u8]) -> Result<()> {
        write_atomic(&self.thumb_path(sha256), jpeg).context("writing thumbnail")
    }

    /// Returns a thumbnail for a record, generating and caching it on first use:
    ///   cached JPEG → image-crate decode (JPEG/PNG/…) → ffmpeg (HEIC/video).
    /// None only if every path fails (e.g. HEIC/video and ffmpeg unavailable).
    /// @param record - the media record to thumbnail
    /// @param ffmpeg - path to the ffmpeg binary for formats the image crate can't decode
    pub fn thumbnail_bytes(&self, record: &MediaRecord, ffmpeg: &str) -> Option<Vec<u8>> {
        let cache_path = self.thumb_path(&record.sha256);
        if let Ok(bytes) = std::fs::read(&cache_path) {
            return Some(bytes);
        }
        let source = self.absolute_path(record);

        let generated = if is_thumbnailable(&record.content_type, &record.filename) {
            std::fs::read(&source)
                .ok()
                .and_then(|original| image::load_from_memory(&original).ok())
                .and_then(|image| {
                    let thumb = image.thumbnail(512, 512);
                    let mut buf = std::io::Cursor::new(Vec::new());
                    thumb.write_to(&mut buf, image::ImageFormat::Jpeg).ok()?;
                    Some(buf.into_inner())
                })
        } else {
            // HEIC stills and videos — decode a frame with ffmpeg.
            ffmpeg_thumbnail(ffmpeg, &source, &record.media_type)
        };

        let bytes = generated?;
        let _ = write_atomic(&cache_path, &bytes);
        Some(bytes)
    }

    /// Resolves the absolute path of a record's bytes on disk, honoring which
    /// root the record was written under.
    pub fn absolute_path(&self, record: &MediaRecord) -> PathBuf {
        match record.storage_root {
            StorageRoot::MediaRoot => self.media_root.join(&record.rel_path),
            StorageRoot::DataDir => self.data_dir.join(&record.rel_path),
        }
    }

    /// Persists uploaded bytes idempotently.
    ///
    /// Content already on disk (matched by sha256) is reused rather than written
    /// again; new content is filed into the month folder derived from the
    /// capture time. Returns the resulting record and whether this upload was a
    /// duplicate of one already recorded for the same asset.
    /// @param asset_id - stable client-side identifier for the source asset
    /// @param filename - original filename from the device
    /// @param content_type - MIME type declared by the client
    /// @param media_type - "photo" or "video"
    /// @param created_at - ISO-8601 capture time from the device
    /// @param bytes - the file contents
    pub fn store(&self, asset_id: &str, filename: &str, content_type: &str, media_type: &str, created_at: &str, bytes: &[u8]) -> Result<(MediaRecord, bool)> {
        let _writing = self.write_lock.lock().unwrap();
        let sha256 = hex::encode(Sha256::digest(bytes));
        let existing = self.stored_copy_of(&sha256);
        let content_existed = existing.is_some();
        let (rel_path, storage_root) = match existing {
            Some(prior) => (prior.rel_path, prior.storage_root),
            None => (
                self.write_new_media(filename, content_type, created_at, &sha256, bytes)?,
                StorageRoot::MediaRoot,
            ),
        };

        let record = MediaRecord {
            asset_id: asset_id.to_string(),
            sha256: sha256.clone(),
            filename: filename.to_string(),
            content_type: content_type.to_string(),
            media_type: media_type.to_string(),
            created_at: created_at.to_string(),
            rel_path,
            storage_root,
            size: bytes.len() as u64,
            ingested_at: chrono::Utc::now().timestamp(),
        };

        let duplicate = {
            let mut index = self.index.lock().unwrap();
            let already_indexed = index.contains_key(asset_id);
            index.insert(asset_id.to_string(), record.clone());
            persist_index(&self.data_dir, &index).context("persisting index")?;
            content_existed && already_indexed
        };

        Ok((record, duplicate))
    }

    /// Directory staging in-progress chunks for a given content hash. Lives
    /// under the data dir (not the media library) so partial uploads never
    /// pollute the photo tree.
    ///
    /// The hash is validated first: it is joined onto the data dir, and
    /// `Path::join` with an absolute or `..`-bearing value silently escapes, so
    /// an unvalidated value would let any signed-in caller choose where chunk
    /// bytes land anywhere on the filesystem.
    /// @param sha256 - the client-supplied content hash
    fn chunk_dir(&self, sha256: &str) -> Result<PathBuf> {
        ensure_valid_content_hash(sha256)?;
        Ok(self.data_dir.join("chunks").join(sha256))
    }

    /// True if the full content for `sha256` is already stored on disk.
    /// @param sha256 - hex content hash
    pub fn is_content_stored(&self, sha256: &str) -> bool {
        is_valid_content_hash(sha256) && self.stored_copy_of(sha256).is_some()
    }

    /// Persists a single chunk of an in-progress chunked upload.
    /// @param sha256 - full-file content hash the chunk belongs to
    /// @param index - zero-based chunk index
    /// @param bytes - the chunk contents
    pub fn write_chunk(&self, sha256: &str, index: u32, bytes: &[u8]) -> Result<()> {
        let path = self.chunk_dir(sha256)?.join(format!("{index}.part"));
        write_atomic(&path, bytes).context("writing chunk")
    }

    /// Lists the chunk indices already received for `sha256`, ascending, so the
    /// client can resume an interrupted upload by sending only what's missing.
    /// An invalid hash simply reports nothing received.
    /// @param sha256 - full-file content hash
    pub fn received_chunk_indices(&self, sha256: &str) -> Vec<u32> {
        let Ok(dir) = self.chunk_dir(sha256) else {
            return Vec::new();
        };
        let mut indices: Vec<u32> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                entry.file_name().to_str()?.strip_suffix(".part")?.parse().ok()
            })
            .collect();
        indices.sort_unstable();
        indices
    }

    /// Assembles previously-uploaded chunks `0..total_chunks` into the final
    /// file, verifying the combined content matches `expected_sha` before
    /// committing. Content already stored is reused (dedup). Chunks are streamed
    /// one at a time, so a multi-GB video is never held whole in memory. The
    /// chunk staging directory is removed on success.
    /// @param asset_id - stable client-side identifier for the source asset
    /// @param filename - original filename from the device
    /// @param content_type - MIME type declared by the client
    /// @param media_type - "photo" or "video"
    /// @param created_at - ISO-8601 capture time deciding the month folder
    /// @param expected_sha - hex sha256 the assembled bytes must match
    /// @param total_chunks - number of chunks that make up the file
    pub fn assemble_and_store(&self, asset_id: &str, filename: &str, content_type: &str, media_type: &str, created_at: &str, expected_sha: &str, total_chunks: u32) -> Result<(MediaRecord, bool)> {
        let chunk_dir = self.chunk_dir(expected_sha)?;
        let _writing = self.write_lock.lock().unwrap();

        // Dedup: if this content is already on disk, just record it for this asset.
        if let Some(prior) = self.stored_copy_of(expected_sha) {
            let record = self.make_record(asset_id, expected_sha, filename, content_type, media_type, created_at, prior.rel_path, prior.storage_root, prior.size);
            let dup = self.index_record(&record)?;
            let _ = std::fs::remove_dir_all(&chunk_dir);
            return Ok((record, dup));
        }

        // Assemble into a temp file in the destination month folder, hashing as
        // we go so we can reject a corrupt/incomplete upload before committing.
        let folder = month_folder(created_at, &self.folder_suffix);
        let dir = self.media_root.join(&folder);
        std::fs::create_dir_all(&dir).context("creating month folder")?;
        let ticket = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = dir.join(format!(".{}-{ticket}.assembling", std::process::id()));

        // Any failure below must take the half-written temp file with it, or a
        // stray `.assembling` file is left sitting in the photo library.
        let size = match concatenate_chunks(&chunk_dir, total_chunks, expected_sha, &tmp) {
            Ok(size) => size,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        };

        let name = unique_filename(&dir, &safe_filename(filename, content_type), expected_sha);
        std::fs::rename(&tmp, dir.join(&name)).context("finalizing assembled file")?;
        let record = self.make_record(asset_id, expected_sha, filename, content_type, media_type, created_at, format!("{folder}/{name}"), StorageRoot::MediaRoot, size);
        let dup = self.index_record(&record)?;
        let _ = std::fs::remove_dir_all(&chunk_dir);
        Ok((record, dup))
    }

    /// Builds a MediaRecord from its parts (shared by store paths).
    #[allow(clippy::too_many_arguments)]
    fn make_record(&self, asset_id: &str, sha256: &str, filename: &str, content_type: &str, media_type: &str, created_at: &str, rel_path: String, storage_root: StorageRoot, size: u64) -> MediaRecord {
        MediaRecord {
            asset_id: asset_id.to_string(),
            sha256: sha256.to_string(),
            filename: filename.to_string(),
            content_type: content_type.to_string(),
            media_type: media_type.to_string(),
            created_at: created_at.to_string(),
            rel_path,
            storage_root,
            size,
            ingested_at: chrono::Utc::now().timestamp(),
        }
    }

    /// Inserts/updates a record in the index and persists it, returning whether
    /// the asset was already indexed (i.e. this submission is a duplicate).
    fn index_record(&self, record: &MediaRecord) -> Result<bool> {
        let mut index = self.index.lock().unwrap();
        let already = index.contains_key(&record.asset_id);
        index.insert(record.asset_id.clone(), record.clone());
        persist_index(&self.data_dir, &index).context("persisting index")?;
        Ok(already)
    }

    /// Finds an indexed record whose bytes are already on disk with the same
    /// content hash, so a re-upload reuses that file instead of writing a copy.
    /// @param sha256 - hex content hash of the incoming bytes
    fn stored_copy_of(&self, sha256: &str) -> Option<MediaRecord> {
        let candidate = self
            .index
            .lock()
            .unwrap()
            .values()
            .find(|r| r.sha256 == sha256)
            .cloned()?;
        self.absolute_path(&candidate)
            .exists()
            .then_some(candidate)
    }

    /// Writes new content into `<media_root>/<year>/<yyyymm>-<suffix>/`, picking a
    /// filename that does not collide with anything already there, and returns
    /// the path relative to the media root.
    /// @param filename - original filename from the device
    /// @param content_type - MIME type, used when the name carries no extension
    /// @param created_at - ISO-8601 capture time deciding the month folder
    /// @param sha256 - content hash, used to disambiguate colliding names
    /// @param bytes - the file contents
    fn write_new_media(&self, filename: &str, content_type: &str, created_at: &str, sha256: &str, bytes: &[u8]) -> Result<String> {
        let folder = month_folder(created_at, &self.folder_suffix);
        let dir = self.media_root.join(&folder);
        std::fs::create_dir_all(&dir).context("creating month folder")?;
        let name = unique_filename(&dir, &safe_filename(filename, content_type), sha256);
        write_atomic(&dir.join(&name), bytes).context("writing media file")?;
        Ok(format!("{folder}/{name}"))
    }
}

/// Generates a ~512px-wide JPEG thumbnail by shelling out to ffmpeg, for the
/// formats the pure-Rust image crate can't decode: HEIC stills and video frames.
/// For videos it seeks ~1s in for a representative frame, falling back to the
/// first frame for very short clips. Returns None if ffmpeg is missing or fails.
/// @param ffmpeg - path to the ffmpeg binary
/// @param source - the media file to thumbnail
/// @param media_type - "photo" or "video"
fn ffmpeg_thumbnail(ffmpeg: &str, source: &Path, media_type: &str) -> Option<Vec<u8>> {
    let ticket = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let out = std::env::temp_dir().join(format!("psync-thumb-{}-{ticket}.jpg", std::process::id()));

    // Decode a single full-resolution frame with no `-vf`: HEIC stills are
    // decoded through an internal complex filtergraph on some ffmpeg builds,
    // which conflicts with a simple `-vf scale`. We downscale afterwards with
    // the image crate instead, which works for the JPEG ffmpeg emits.
    let run = |seek: Option<&str>| -> bool {
        let mut cmd = std::process::Command::new(ffmpeg);
        cmd.arg("-y").arg("-loglevel").arg("error");
        if let Some(s) = seek {
            cmd.arg("-ss").arg(s);
        }
        cmd.arg("-i").arg(source).arg("-frames:v").arg("1").arg(&out);
        matches!(cmd.output(), Ok(o) if o.status.success())
            && std::fs::metadata(&out).map(|m| m.len() > 0).unwrap_or(false)
    };

    let ok = if media_type == "video" {
        run(Some("00:00:01")) || run(None)
    } else {
        run(None)
    };
    if !ok {
        let _ = std::fs::remove_file(&out);
        return None;
    }

    let full = std::fs::read(&out).ok();
    let _ = std::fs::remove_file(&out);
    let image = image::load_from_memory(&full?).ok()?;
    let thumb = image.thumbnail(512, 512);
    let mut buf = std::io::Cursor::new(Vec::new());
    thumb.write_to(&mut buf, image::ImageFormat::Jpeg).ok()?;
    Some(buf.into_inner())
}

/// Reports whether an item can be decoded into an image thumbnail by the
/// pure-Rust `image` crate (JPEG/PNG/GIF/WebP/BMP/TIFF). HEIC and videos are
/// handled via ffmpeg instead.
pub fn is_thumbnailable(content_type: &str, filename: &str) -> bool {
    let ct = content_type.to_lowercase();
    if ct == "image/jpeg" || ct == "image/png" || ct == "image/gif" || ct == "image/webp" || ct == "image/bmp" || ct == "image/tiff" {
        return true;
    }
    let name = filename.to_lowercase();
    [".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp", ".tif", ".tiff"]
        .iter()
        .any(|ext| name.ends_with(ext))
}

/// Streams chunks `0..total_chunks` into `destination`, one at a time so a
/// multi-GB video is never held whole in memory, and verifies the combined
/// content hashes to `expected_sha`. Returns the assembled byte count.
/// @param chunk_dir - staging directory holding the `<index>.part` files
/// @param total_chunks - number of chunks that make up the file
/// @param expected_sha - hex sha256 the assembled bytes must match
/// @param destination - temp file the assembled bytes are written to
fn concatenate_chunks(chunk_dir: &Path, total_chunks: u32, expected_sha: &str, destination: &Path) -> Result<u64> {
    let mut hasher = Sha256::new();
    let mut size: u64 = 0;
    {
        let mut out = std::fs::File::create(destination).context("creating assembly temp")?;
        for index in 0..total_chunks {
            let data = std::fs::read(chunk_dir.join(format!("{index}.part")))
                .with_context(|| format!("missing chunk {index}"))?;
            hasher.update(&data);
            out.write_all(&data)?;
            size += data.len() as u64;
        }
        out.flush()?;
        out.sync_all()?;
    }
    if hex::encode(hasher.finalize()) != expected_sha {
        anyhow::bail!("assembled sha256 does not match declared hash");
    }
    Ok(size)
}

/// Reports whether a client-supplied content hash is a plain 64-character hex
/// sha256. Values that fail this are never used to build a filesystem path.
/// @param sha256 - the value the client sent
pub fn is_valid_content_hash(sha256: &str) -> bool {
    sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Rejects a content hash that could escape the staging directory.
/// @param sha256 - the value the client sent
fn ensure_valid_content_hash(sha256: &str) -> Result<()> {
    if !is_valid_content_hash(sha256) {
        anyhow::bail!("sha256 must be 64 hex characters");
    }
    Ok(())
}

/// Builds the `<year>/<yyyymm>-<suffix>` folder for a capture timestamp,
/// falling back to today when the timestamp is missing or unparseable.
/// @param created_at - ISO-8601 capture time from the device
/// @param suffix - suffix appended to the month folder name
pub fn month_folder(created_at: &str, suffix: &str) -> String {
    let date = capture_date(created_at).unwrap_or_else(|| Local::now().date_naive());
    format!("{}/{}-{}", date.format("%Y"), date.format("%Y%m"), suffix)
}

/// Parses the client-supplied capture timestamp into a calendar date.
///
/// The iOS app sends RFC-3339 in UTC; that is converted to this machine's local
/// time first, so a photo taken at 8pm on August 31st files under August rather
/// than slipping into September with the UTC date. Plain `YYYY-MM-DDTHH:MM:SS`
/// and bare `YYYY-MM-DD` are also accepted.
/// @param created_at - the timestamp string to parse
fn capture_date(created_at: &str) -> Option<NaiveDate> {
    let trimmed = created_at.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Some(dt.with_timezone(&Local).date_naive());
    }
    for format in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(trimmed, format) {
            return Some(dt.date());
        }
    }
    let head: String = trimmed.chars().take(10).collect();
    NaiveDate::parse_from_str(&head, "%Y-%m-%d").ok()
}

/// Reduces a client-supplied filename to a safe Windows-legal base name,
/// appending an extension derived from the content type when it has none.
/// @param filename - the raw filename from the client
/// @param content_type - MIME type used to pick a fallback extension
fn safe_filename(filename: &str, content_type: &str) -> String {
    let base = filename.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control() && !"<>:\"|?*".contains(*c))
        .collect();
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace()).to_string();
    if cleaned.is_empty() {
        return format!("media.{}", extension_for("", content_type));
    }
    if Path::new(&cleaned).extension().is_none() {
        return format!("{cleaned}.{}", extension_for("", content_type));
    }
    cleaned
}

/// Picks a filename that does not collide with an existing file in `dir`.
/// Different phones (and different months of the same phone) both emit
/// `IMG_0001.jpg`, so a colliding name is disambiguated with a short content
/// hash rather than silently overwriting a photo.
/// @param dir - the month folder the file will be written into
/// @param name - the preferred filename
/// @param sha256 - content hash used as the disambiguator
fn unique_filename(dir: &Path, name: &str, sha256: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or_default();
    for discriminator in [&sha256[..8], sha256] {
        let candidate = if ext.is_empty() {
            format!("{stem}-{discriminator}")
        } else {
            format!("{stem}-{discriminator}.{ext}")
        };
        if !dir.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("{stem}-{}.{ext}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default())
}

/// Chooses a file extension from the filename, falling back to the content type.
fn extension_for(filename: &str, content_type: &str) -> String {
    if let Some(ext) = Path::new(filename).extension().and_then(|e| e.to_str()) {
        return ext.to_lowercase();
    }
    match content_type {
        "image/jpeg" => "jpg".into(),
        "image/png" => "png".into(),
        "image/heic" => "heic".into(),
        "video/quicktime" => "mov".into(),
        "video/mp4" => "mp4".into(),
        _ => "bin".into(),
    }
}

/// Writes bytes atomically: to a uniquely-named temp file in the destination
/// directory, flush+sync, then rename into place. The temp name carries a
/// process-unique counter so two writes in the same folder never share one.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let ticket = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("{}-{ticket}.tmp-upload", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.flush()?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Absolute path of the metadata index JSON file.
fn index_file(data_dir: &Path) -> PathBuf {
    data_dir.join("index").join("manifest.json")
}

/// Loads the metadata index from disk, if it exists.
fn load_index(data_dir: &Path) -> Option<HashMap<String, MediaRecord>> {
    let path = index_file(data_dir);
    let data = std::fs::read(path).ok()?;
    serde_json::from_slice(&data).ok()
}

/// Persists the metadata index atomically.
fn persist_index(data_dir: &Path, index: &HashMap<String, MediaRecord>) -> Result<()> {
    let json = serde_json::to_vec_pretty(index)?;
    write_atomic(&index_file(data_dir), &json)
}
