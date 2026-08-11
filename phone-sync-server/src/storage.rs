//! Filesystem-backed media storage with a JSON metadata index.
//!
//! Media bytes are content-addressed: the file path is derived from the
//! sha256 of the content, so identical bytes are stored once (idempotency).
//! A JSON index maps asset ids -> records and is persisted atomically.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::models::MediaRecord;

/// Owns the data directory and an in-memory index guarded by a mutex.
pub struct Storage {
    data_dir: PathBuf,
    /// asset_id -> record. Protected for concurrent uploads.
    index: Mutex<HashMap<String, MediaRecord>>,
}

impl Storage {
    /// Opens (or initializes) storage rooted at `data_dir`, loading any
    /// existing metadata index from disk.
    pub fn open(data_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(data_dir.join("media"))
            .context("creating media dir")?;
        std::fs::create_dir_all(data_dir.join("index"))
            .context("creating index dir")?;
        std::fs::create_dir_all(data_dir.join("thumbs"))
            .context("creating thumbs dir")?;
        let index = load_index(&data_dir).unwrap_or_default();
        Ok(Self {
            data_dir,
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

    /// Returns a cached (or freshly generated) JPEG thumbnail for an image
    /// record, or None if the format can't be decoded (e.g. HEIC/video).
    /// Thumbnails are ~512px on the long edge and cached under `thumbs/`.
    pub fn thumbnail_bytes(&self, record: &MediaRecord) -> Option<Vec<u8>> {
        if !is_thumbnailable(&record.content_type, &record.filename) {
            return None;
        }
        let cache_path = self.data_dir.join("thumbs").join(format!("{}.jpg", record.sha256));
        if let Ok(bytes) = std::fs::read(&cache_path) {
            return Some(bytes);
        }
        let original = std::fs::read(self.absolute_path(record)).ok()?;
        let image = image::load_from_memory(&original).ok()?;
        let thumb = image.thumbnail(512, 512);
        let mut buf = std::io::Cursor::new(Vec::new());
        thumb.write_to(&mut buf, image::ImageFormat::Jpeg).ok()?;
        let bytes = buf.into_inner();
        let _ = write_atomic(&cache_path, &bytes);
        Some(bytes)
    }

    /// Resolves the absolute path of a record's bytes on disk.
    pub fn absolute_path(&self, record: &MediaRecord) -> PathBuf {
        self.data_dir.join(&record.rel_path)
    }

    /// Persists uploaded bytes idempotently.
    ///
    /// Computes the sha256 of `bytes`, writes them to a content-addressed path
    /// (atomically via temp file + rename) unless already present, records the
    /// metadata keyed by `asset_id`, and returns (record, was_duplicate).
    pub fn store(&self, asset_id: &str, filename: &str, content_type: &str, media_type: &str, created_at: &str, bytes: &[u8]) -> Result<(MediaRecord, bool)> {
        let sha256 = hex::encode(Sha256::digest(bytes));
        let ext = extension_for(filename, content_type);
        let rel_path = content_path(&sha256, &ext);
        let abs_path = self.data_dir.join(&rel_path);

        let file_existed = abs_path.exists();
        if !file_existed {
            write_atomic(&abs_path, bytes).context("writing media file")?;
        }

        let record = MediaRecord {
            asset_id: asset_id.to_string(),
            sha256: sha256.clone(),
            filename: filename.to_string(),
            content_type: content_type.to_string(),
            media_type: media_type.to_string(),
            created_at: created_at.to_string(),
            rel_path: rel_path.to_string_lossy().to_string(),
            size: bytes.len() as u64,
            ingested_at: chrono::Utc::now().timestamp(),
        };

        let duplicate = {
            let mut index = self.index.lock().unwrap();
            let already_indexed = index.contains_key(asset_id);
            index.insert(asset_id.to_string(), record.clone());
            persist_index(&self.data_dir, &index).context("persisting index")?;
            file_existed && already_indexed
        };

        Ok((record, duplicate))
    }
}

/// Reports whether an item can be decoded into an image thumbnail by the
/// pure-Rust `image` crate (JPEG/PNG/GIF/WebP/BMP/TIFF). HEIC and videos can't
/// be decoded here and fall back to a client-side treatment.
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

/// Derives a two-level content-addressed relative path from a sha256 hash,
/// e.g. `media/ab/abcd...ef.jpg`, to avoid huge flat directories.
fn content_path(sha256: &str, ext: &str) -> PathBuf {
    let prefix = &sha256[0..2];
    if ext.is_empty() {
        PathBuf::from("media").join(prefix).join(sha256)
    } else {
        PathBuf::from("media").join(prefix).join(format!("{sha256}.{ext}"))
    }
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

/// Writes bytes atomically: to a temp file, flush+sync, then rename into place.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp-upload");
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
