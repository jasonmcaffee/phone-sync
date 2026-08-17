//! End-to-end integration tests: spin up the real axum app on an ephemeral
//! port and exercise it over HTTP with reqwest. Favors functional coverage of
//! the auth + upload + manifest flows over isolated unit tests.

use std::sync::Arc;

use phone_sync_server::auth;
use phone_sync_server::config::Config;
use phone_sync_server::state::AppState;
use phone_sync_server::storage::Storage;
use phone_sync_server::build_app;
use serde_json::Value;

/// Boots the app against a temp data dir on a random port and returns the base
/// URL plus the tempdir guard (kept alive for the test's duration).
async fn spawn_server() -> (String, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let config = Config {
        bind_addr: "127.0.0.1:0".into(),
        data_dir: tmp.path().to_path_buf(),
        media_root: tmp.path().join("pictures"),
        media_folder_suffix: "phone-sync".into(),
        username: "jason".into(),
        password_hash: auth::hash_password("modestMouse1!"),
        jwt_secret: "test-secret".into(),
        token_ttl_secs: 365 * 24 * 60 * 60,
        max_upload_bytes: 10 * 1024 * 1024,
        ffmpeg_path: "ffmpeg".into(),
        ffprobe_path: "ffprobe".into(),
        thumbnail_max_dim: 512,
        preview_max_dim: 2048,
        default_page_size: 120,
        max_page_size: 500,
        thumbnail_workers: 2,
    };
    let storage = Storage::open(
        config.data_dir.clone(),
        config.media_root.clone(),
        config.media_folder_suffix.clone(),
    )
    .unwrap();
    let state = AppState {
        config: Arc::new(config),
        storage: Arc::new(storage),
    };
    let app = build_app(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), tmp)
}

/// Logs in with the seeded credentials and returns the bearer token.
async fn login(base: &str, client: &reqwest::Client) -> String {
    let resp = client
        .post(format!("{base}/auth/login"))
        .json(&serde_json::json!({"username":"jason","password":"modestMouse1!"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "login should succeed");
    let body: Value = resp.json().await.unwrap();
    body["token"].as_str().unwrap().to_string()
}

/// Builds a multipart upload form for a fake asset.
fn upload_form(asset_id: &str, filename: &str, content_type: &str, media_type: &str, bytes: Vec<u8>) -> reqwest::multipart::Form {
    upload_form_at(asset_id, filename, content_type, media_type, "2026-08-11T18:00:00Z", bytes)
}

/// Builds a multipart upload form with an explicit capture timestamp, so tests
/// can assert which month folder an item is filed into.
fn upload_form_at(asset_id: &str, filename: &str, content_type: &str, media_type: &str, created_at: &str, bytes: Vec<u8>) -> reqwest::multipart::Form {
    use sha2::{Digest, Sha256};
    let sha = hex::encode(Sha256::digest(&bytes));
    let meta = serde_json::json!({
        "asset_id": asset_id,
        "filename": filename,
        "content_type": content_type,
        "created_at": created_at,
        "media_type": media_type,
        "sha256": sha,
    });
    let file_part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename.to_string())
        .mime_str(content_type)
        .unwrap();
    reqwest::multipart::Form::new()
        .text("metadata", meta.to_string())
        .part("file", file_part)
}

#[tokio::test]
async fn health_is_ok() {
    let (base, _tmp) = spawn_server().await;
    let client = reqwest::Client::new();
    let resp = client.get(format!("{base}/health")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn login_succeeds_and_rejects_bad_password() {
    let (base, _tmp) = spawn_server().await;
    let client = reqwest::Client::new();

    let ok = client
        .post(format!("{base}/auth/login"))
        .json(&serde_json::json!({"username":"jason","password":"modestMouse1!"}))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    let body: Value = ok.json().await.unwrap();
    assert!(body["token"].as_str().unwrap().len() > 10);
    assert!(body["expires_at"].as_i64().unwrap() > chrono::Utc::now().timestamp());

    let bad = client
        .post(format!("{base}/auth/login"))
        .json(&serde_json::json!({"username":"jason","password":"wrong"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 401);
}

#[tokio::test]
async fn protected_routes_require_bearer() {
    let (base, _tmp) = spawn_server().await;
    let client = reqwest::Client::new();

    let no_auth = client.get(format!("{base}/media/manifest")).send().await.unwrap();
    assert_eq!(no_auth.status(), 401);

    let bad_auth = client
        .get(format!("{base}/media/manifest"))
        .header("Authorization", "Bearer not-a-real-token")
        .send()
        .await
        .unwrap();
    assert_eq!(bad_auth.status(), 401);
}

#[tokio::test]
async fn upload_stores_and_is_idempotent() {
    let (base, _tmp) = spawn_server().await;
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    let bytes = b"fake-jpeg-bytes-hello-world".to_vec();
    let form = upload_form("asset-1", "IMG_0001.jpg", "image/jpeg", "photo", bytes.clone());
    let resp = client
        .post(format!("{base}/media/upload"))
        .bearer_auth(&token)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201, "upload status");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["stored"], true);
    assert_eq!(body["duplicate"], false);

    // Re-upload identical bytes -> duplicate, no new file.
    let form2 = upload_form("asset-1", "IMG_0001.jpg", "image/jpeg", "photo", bytes.clone());
    let resp2 = client
        .post(format!("{base}/media/upload"))
        .bearer_auth(&token)
        .multipart(form2)
        .send()
        .await
        .unwrap();
    let body2: Value = resp2.json().await.unwrap();
    assert_eq!(body2["duplicate"], true, "second identical upload is a duplicate");

    // Manifest lists exactly the one asset.
    let manifest: Value = client
        .get(format!("{base}/media/manifest"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(manifest["count"], 1);
    let ids: Vec<String> = manifest["asset_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, vec!["asset-1".to_string()]);
}

#[tokio::test]
async fn upload_photo_and_video_then_fetch() {
    let (base, _tmp) = spawn_server().await;
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    let photo = upload_form("photo-1", "IMG_1.jpg", "image/jpeg", "photo", b"photo-bytes".to_vec());
    let vresp = client.post(format!("{base}/media/upload")).bearer_auth(&token).multipart(photo).send().await.unwrap();
    let vbody: Value = vresp.json().await.unwrap();
    let photo_id = vbody["id"].as_str().unwrap().to_string();

    let video = upload_form("video-1", "MOV_1.mov", "video/quicktime", "video", b"video-bytes".to_vec());
    client.post(format!("{base}/media/upload")).bearer_auth(&token).multipart(video).send().await.unwrap();

    // Manifest should list both.
    let manifest: Value = client
        .get(format!("{base}/media/manifest"))
        .bearer_auth(&token)
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(manifest["count"], 2);

    // Fetch the photo bytes back.
    let fetched = client
        .get(format!("{base}/media/{photo_id}"))
        .bearer_auth(&token)
        .send().await.unwrap();
    assert_eq!(fetched.status(), 200);
    let fetched_bytes = fetched.bytes().await.unwrap();
    assert_eq!(&fetched_bytes[..], b"photo-bytes");
}

/// Encodes a small solid-color PNG for gallery/thumbnail tests.
fn make_png() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(16, 16, image::Rgb([120, 60, 200]));
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut buf, image::ImageFormat::Png)
        .unwrap();
    buf.into_inner()
}

#[tokio::test]
async fn gallery_page_is_served_publicly() {
    let (base, _tmp) = spawn_server().await;
    let client = reqwest::Client::new();
    let resp = client.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("Phone Sync"));
    assert!(body.contains("id=\"grid\""));
}

#[tokio::test]
async fn list_thumbnail_and_query_token_auth() {
    let (base, _tmp) = spawn_server().await;
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    let png = make_png();
    let form = upload_form("gallery-1", "pic.png", "image/png", "photo", png);
    client.post(format!("{base}/media/upload")).bearer_auth(&token).multipart(form).send().await.unwrap();

    // Listing returns the item and marks it thumbnailable.
    let list: Value = client.get(format!("{base}/api/media")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    assert_eq!(list["count"], 1);
    let id = list["items"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(list["items"][0]["thumbnailable"], true);

    // Thumbnail served as JPEG, authenticated via ?token= query (no header).
    let thumb = client.get(format!("{base}/media/{id}/thumb?token={token}")).send().await.unwrap();
    assert_eq!(thumb.status(), 200);
    assert_eq!(thumb.headers()["content-type"], "image/jpeg");
    assert!(!thumb.bytes().await.unwrap().is_empty());

    // A bad query token is rejected.
    let bad = client.get(format!("{base}/media/{id}/thumb?token=nope")).send().await.unwrap();
    assert_eq!(bad.status(), 401);
}

#[tokio::test]
async fn media_supports_range_requests() {
    let (base, _tmp) = spawn_server().await;
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    let png = make_png();
    let total = png.len();
    let form = upload_form("range-1", "pic.png", "image/png", "photo", png);
    let up: Value = client.post(format!("{base}/media/upload")).bearer_auth(&token).multipart(form).send().await.unwrap().json().await.unwrap();
    let id = up["id"].as_str().unwrap().to_string();

    let resp = client
        .get(format!("{base}/media/{id}"))
        .bearer_auth(&token)
        .header("Range", "bytes=0-9")
        .send().await.unwrap();
    assert_eq!(resp.status(), 206);
    assert_eq!(resp.headers()["content-range"], format!("bytes 0-9/{total}"));
    assert_eq!(resp.bytes().await.unwrap().len(), 10);
}

#[tokio::test]
async fn an_open_ended_range_streams_from_the_requested_offset() {
    let (base, _tmp) = spawn_server().await;
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    // Distinct bytes, so a wrong offset produces a wrong body rather than
    // accidentally matching.
    let content: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
    let total = content.len();
    let form = upload_form("stream-1", "VID.mp4", "video/mp4", "video", content.clone());
    let up: Value = client.post(format!("{base}/media/upload")).bearer_auth(&token).multipart(form).send().await.unwrap().json().await.unwrap();
    let id = up["id"].as_str().unwrap().to_string();

    // The open-ended form a browser uses once it decides to keep playing.
    let resp = client.get(format!("{base}/media/{id}")).bearer_auth(&token).header("Range", "bytes=10000-").send().await.unwrap();
    assert_eq!(resp.status(), 206);
    assert_eq!(resp.headers()["content-range"], format!("bytes 10000-{}/{total}", total - 1));
    assert_eq!(resp.bytes().await.unwrap().to_vec(), content[10_000..], "streamed from the right offset");

    // The two-byte probe Safari opens a video with.
    let probe = client.get(format!("{base}/media/{id}")).bearer_auth(&token).header("Range", "bytes=0-1").send().await.unwrap();
    assert_eq!(probe.status(), 206);
    assert_eq!(probe.headers()["accept-ranges"], "bytes");
    assert_eq!(probe.bytes().await.unwrap().to_vec(), content[..2]);

    // A range past the end is refused, not silently answered with everything.
    let beyond = client.get(format!("{base}/media/{id}")).bearer_auth(&token).header("Range", "bytes=999999-1000000").send().await.unwrap();
    assert_eq!(beyond.status(), 416);
    assert_eq!(beyond.headers()["content-range"], format!("bytes */{total}"));
}

#[tokio::test]
async fn iphone_video_is_served_with_its_true_mp4_family_type() {
    let (base, _tmp) = spawn_server().await;
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    // Uploaded as video/quicktime, exactly as an iPhone .MOV arrives.
    let form = upload_form("mov-1", "IMG_5424.MOV", "video/quicktime", "video", b"quicktime-bytes".to_vec());
    let up: Value = client.post(format!("{base}/media/upload")).bearer_auth(&token).multipart(form).send().await.unwrap().json().await.unwrap();
    let id = up["id"].as_str().unwrap().to_string();

    let resp = client.get(format!("{base}/media/{id}")).bearer_auth(&token).send().await.unwrap();
    assert_eq!(resp.headers()["content-type"], "video/mp4", "canPlayType reports nothing for video/quicktime");

    // The listing tells the client the same thing up front.
    let list: Value = client.get(format!("{base}/api/media")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    let item = list["items"].as_array().unwrap().iter().find(|i| i["id"] == id).unwrap();
    assert_eq!(item["served_content_type"], "video/mp4");
    assert_eq!(item["content_type"], "video/quicktime", "the record keeps what was uploaded");
}

#[tokio::test]
async fn media_is_cached_immutably_and_answers_conditional_requests() {
    let (base, _tmp) = spawn_server().await;
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    let png = make_png();
    let form = upload_form("cache-1", "pic.png", "image/png", "photo", png);
    let up: Value = client.post(format!("{base}/media/upload")).bearer_auth(&token).multipart(form).send().await.unwrap().json().await.unwrap();
    let id = up["id"].as_str().unwrap().to_string();

    for path in [format!("/media/{id}"), format!("/media/{id}/thumb"), format!("/media/{id}/preview")] {
        let first = client.get(format!("{base}{path}")).bearer_auth(&token).send().await.unwrap();
        assert_eq!(first.status(), 200, "{path}");
        let cache_control = first.headers()["cache-control"].to_str().unwrap().to_string();
        assert!(cache_control.contains("immutable"), "{path} should be immutable, got {cache_control}");
        let etag = first.headers()["etag"].to_str().unwrap().to_string();

        // Re-requesting with the tag we were given costs nothing.
        let again = client.get(format!("{base}{path}")).bearer_auth(&token).header("If-None-Match", &etag).send().await.unwrap();
        assert_eq!(again.status(), 304, "{path} should be a cache hit");
        assert!(again.bytes().await.unwrap().is_empty());
    }

    // The original and its renditions must not share an ETag, or a cache could
    // serve a thumbnail in place of the full image.
    let tag_of = |path: String| {
        let client = client.clone();
        let base = base.clone();
        let token = token.clone();
        async move {
            client.get(format!("{base}{path}")).bearer_auth(&token).send().await.unwrap().headers()["etag"].to_str().unwrap().to_string()
        }
    };
    let original = tag_of(format!("/media/{id}")).await;
    let thumb = tag_of(format!("/media/{id}/thumb")).await;
    let preview = tag_of(format!("/media/{id}/preview")).await;
    assert_ne!(original, thumb);
    assert_ne!(thumb, preview);
}

#[tokio::test]
async fn the_listing_is_paged_newest_first() {
    let (base, _tmp) = spawn_server().await;
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    // Twelve captures on consecutive days, uploaded oldest-first.
    for day in 1..=12u32 {
        let created_at = format!("2026-03-{day:02}T12:00:00Z");
        let form = upload_form_at(&format!("page-{day}"), &format!("IMG_{day:04}.jpg"), "image/jpeg", "photo", &created_at, format!("bytes-{day}").into_bytes());
        client.post(format!("{base}/media/upload")).bearer_auth(&token).multipart(form).send().await.unwrap();
    }

    let page_one: Value = client.get(format!("{base}/api/media?offset=0&limit=5")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    assert_eq!(page_one["count"], 12, "count is the whole library, not the page");
    assert_eq!(page_one["offset"], 0);
    assert_eq!(page_one["limit"], 5);
    assert_eq!(page_one["items"].as_array().unwrap().len(), 5);
    assert_eq!(page_one["items"][0]["filename"], "IMG_0012.jpg", "newest first");

    let page_three: Value = client.get(format!("{base}/api/media?offset=10&limit=5")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    assert_eq!(page_three["items"].as_array().unwrap().len(), 2, "final partial page");
    assert_eq!(page_three["items"][1]["filename"], "IMG_0001.jpg", "oldest last");

    // Walking every page must visit each item exactly once — no repeats, none skipped.
    let mut seen: Vec<String> = Vec::new();
    for offset in (0..12).step_by(5) {
        let page: Value = client.get(format!("{base}/api/media?offset={offset}&limit=5")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
        seen.extend(page["items"].as_array().unwrap().iter().map(|i| i["filename"].as_str().unwrap().to_string()));
    }
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(seen.len(), 12, "paging visited every item");
    assert_eq!(unique.len(), 12, "paging visited none of them twice");

    // An absurd page size is clamped rather than honoured.
    let huge: Value = client.get(format!("{base}/api/media?limit=100000")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    assert_eq!(huge["limit"], 500);
}

#[tokio::test]
async fn heic_is_flagged_as_needing_a_server_rendered_preview() {
    let (base, _tmp) = spawn_server().await;
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    let cases = [
        ("heic-flag", "IMG_1.HEIC", "image/heic", "photo", false),
        ("png-flag", "shot.png", "image/png", "photo", true),
        ("jpg-flag", "IMG_2.jpg", "image/jpeg", "photo", true),
        ("mov-flag", "IMG_3.MOV", "video/quicktime", "video", false),
    ];
    for (asset, filename, content_type, media_type, _) in cases {
        let form = upload_form(asset, filename, content_type, media_type, format!("bytes-for-{asset}").into_bytes());
        client.post(format!("{base}/media/upload")).bearer_auth(&token).multipart(form).send().await.unwrap();
    }

    let list: Value = client.get(format!("{base}/api/media")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    for (asset, _, _, _, displayable) in cases {
        let item = list["items"].as_array().unwrap().iter().find(|i| i["asset_id"] == asset).unwrap();
        assert_eq!(item["browser_displayable"], displayable, "{asset}");
    }
}

#[tokio::test]
async fn upload_rejects_sha_mismatch() {
    let (base, _tmp) = spawn_server().await;
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    let meta = serde_json::json!({
        "asset_id": "bad-sha",
        "filename": "x.jpg",
        "content_type": "image/jpeg",
        "created_at": "2026-08-11T00:00:00Z",
        "media_type": "photo",
        "sha256": "deadbeef",
    });
    let file_part = reqwest::multipart::Part::bytes(b"real-bytes".to_vec())
        .file_name("x.jpg")
        .mime_str("image/jpeg")
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .text("metadata", meta.to_string())
        .part("file", file_part);

    let resp = client
        .post(format!("{base}/media/upload"))
        .bearer_auth(&token)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn media_is_filed_into_year_and_month_folders() {
    let (base, tmp) = spawn_server().await;
    let pictures = tmp.path().join("pictures");
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    // Three captures across two years and three months.
    let captures = [
        ("aug", "IMG_0042.jpg", "2026-08-11T18:00:00Z", "2026/202608-phone-sync"),
        ("jan", "IMG_0100.jpg", "2026-01-05T20:00:00Z", "2026/202601-phone-sync"),
        ("dec", "MOV_0007.mov", "2025-12-25T20:00:00Z", "2025/202512-phone-sync"),
    ];
    for (asset, filename, created_at, expected_folder) in captures {
        let form = upload_form_at(asset, filename, "image/jpeg", "photo", created_at, format!("bytes-for-{asset}").into_bytes());
        let resp = client.post(format!("{base}/media/upload")).bearer_auth(&token).multipart(form).send().await.unwrap();
        assert_eq!(resp.status(), 201, "{asset} upload");
        let on_disk = pictures.join(expected_folder).join(filename);
        assert!(on_disk.exists(), "expected {} to exist", on_disk.display());
    }

    // Nothing was written into the legacy content-addressed tree.
    assert!(!tmp.path().join("media").exists(), "legacy media/ tree should not be used");

    // The listing reports the on-disk location so the gallery can show it.
    let list: Value = client.get(format!("{base}/api/media")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    assert_eq!(list["count"], 3);
    let paths: Vec<String> = list["items"].as_array().unwrap().iter().map(|i| i["rel_path"].as_str().unwrap().to_string()).collect();
    assert!(paths.contains(&"2026/202608-phone-sync/IMG_0042.jpg".to_string()), "got {paths:?}");
}

#[tokio::test]
async fn same_filename_different_content_does_not_overwrite() {
    let (base, tmp) = spawn_server().await;
    let pictures = tmp.path().join("pictures");
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    let first = upload_form_at("a", "IMG_0001.jpg", "image/jpeg", "photo", "2026-08-11T18:00:00Z", b"first-photo".to_vec());
    client.post(format!("{base}/media/upload")).bearer_auth(&token).multipart(first).send().await.unwrap();
    let second = upload_form_at("b", "IMG_0001.jpg", "image/jpeg", "photo", "2026-08-11T18:00:00Z", b"second-photo".to_vec());
    let resp: Value = client.post(format!("{base}/media/upload")).bearer_auth(&token).multipart(second).send().await.unwrap().json().await.unwrap();
    assert_eq!(resp["duplicate"], false);

    let folder = pictures.join("2026/202608-phone-sync");
    let names: Vec<String> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    assert_eq!(names.len(), 2, "both photos kept, got {names:?}");
    assert_eq!(std::fs::read(folder.join("IMG_0001.jpg")).unwrap(), b"first-photo", "original untouched");

    // Both are fetchable and return their own bytes.
    let list: Value = client.get(format!("{base}/api/media")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    for item in list["items"].as_array().unwrap() {
        let id = item["id"].as_str().unwrap();
        let fetched = client.get(format!("{base}/media/{id}")).bearer_auth(&token).send().await.unwrap();
        assert_eq!(fetched.status(), 200);
    }
}

#[tokio::test]
async fn identical_bytes_are_stored_once_on_disk() {
    let (base, tmp) = spawn_server().await;
    let pictures = tmp.path().join("pictures");
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    // The same image imported under two different asset ids / names.
    for (asset, filename) in [("one", "IMG_9.jpg"), ("two", "IMG_9-copy.jpg")] {
        let form = upload_form_at(asset, filename, "image/jpeg", "photo", "2026-08-11T18:00:00Z", b"identical-bytes".to_vec());
        client.post(format!("{base}/media/upload")).bearer_auth(&token).multipart(form).send().await.unwrap();
    }

    let folder = pictures.join("2026/202608-phone-sync");
    let names: Vec<String> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
    assert_eq!(names, vec!["IMG_9.jpg".to_string()], "de-duplicated by content");
}

#[tokio::test]
async fn unparseable_capture_time_falls_back_to_today() {
    let (base, tmp) = spawn_server().await;
    let pictures = tmp.path().join("pictures");
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    let form = upload_form_at("no-date", "IMG_5.jpg", "image/jpeg", "photo", "", b"undated".to_vec());
    let resp = client.post(format!("{base}/media/upload")).bearer_auth(&token).multipart(form).send().await.unwrap();
    assert_eq!(resp.status(), 201);

    let today = chrono::Local::now();
    let expected = pictures
        .join(today.format("%Y").to_string())
        .join(format!("{}-phone-sync", today.format("%Y%m")))
        .join("IMG_5.jpg");
    assert!(expected.exists(), "expected {} to exist", expected.display());
}

/// Sends one chunk of a chunked upload and returns the parsed ack JSON.
async fn send_chunk(base: &str, client: &reqwest::Client, token: &str, sha: &str, index: u32, bytes: &[u8]) -> Value {
    let meta = serde_json::json!({ "sha256": sha, "chunk_index": index });
    let part = reqwest::multipart::Part::bytes(bytes.to_vec()).file_name("chunk");
    let form = reqwest::multipart::Form::new()
        .text("metadata", meta.to_string())
        .part("file", part);
    client
        .post(format!("{base}/media/upload/chunk"))
        .bearer_auth(token)
        .multipart(form)
        .send().await.unwrap()
        .json().await.unwrap()
}

#[tokio::test]
async fn chunked_upload_assembles_verifies_and_resumes() {
    use sha2::{Digest, Sha256};
    let (base, _tmp) = spawn_server().await;
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    // A larger payload split into 3 chunks.
    let content: Vec<u8> = (0..40_000u32).flat_map(|i| i.to_le_bytes()).collect(); // 160 KB
    let sha = hex::encode(Sha256::digest(&content));
    let chunk_size = 60_000usize;
    let chunks: Vec<&[u8]> = content.chunks(chunk_size).collect();
    let total = chunks.len() as u32;

    // Status before anything: not stored, nothing received.
    let st: Value = client.get(format!("{base}/media/upload/status/{sha}")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    assert_eq!(st["stored"], false);
    assert_eq!(st["received"].as_array().unwrap().len(), 0);

    // Upload chunks 0 and 2 first (skip 1), simulating an interruption.
    assert_eq!(send_chunk(&base, &client, &token, &sha, 0, chunks[0]).await["ok"], true);
    assert_eq!(send_chunk(&base, &client, &token, &sha, 2, chunks[2]).await["ok"], true);

    // Status now reports exactly the received indices, so the client can resume.
    let st2: Value = client.get(format!("{base}/media/upload/status/{sha}")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    let received: Vec<u64> = st2["received"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    assert_eq!(received, vec![0, 2]);

    // Send the missing chunk, then finalize.
    send_chunk(&base, &client, &token, &sha, 1, chunks[1]).await;
    let complete: Value = client
        .post(format!("{base}/media/upload/complete"))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "asset_id": "big-video-1", "filename": "VID_9999.mov",
            "content_type": "video/quicktime", "created_at": "2026-08-11T18:00:00Z",
            "media_type": "video", "sha256": sha, "total_chunks": total,
        }))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(complete["stored"], true);
    assert_eq!(complete["id"].as_str().unwrap(), sha);

    // The assembled bytes match the original exactly.
    let got = client.get(format!("{base}/media/{sha}")).bearer_auth(&token).send().await.unwrap().bytes().await.unwrap();
    assert_eq!(&got[..], &content[..]);

    // Content is now marked stored (so a re-sync skips it entirely).
    let st3: Value = client.get(format!("{base}/media/upload/status/{sha}")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    assert_eq!(st3["stored"], true);
}

#[tokio::test]
async fn complete_rejects_content_that_fails_hash_check() {
    use sha2::{Digest, Sha256};
    let (base, _tmp) = spawn_server().await;
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    let content = b"the real bytes".to_vec();
    let wrong_sha = hex::encode(Sha256::digest(b"different bytes"));
    send_chunk(&base, &client, &token, &wrong_sha, 0, &content).await;

    let resp = client
        .post(format!("{base}/media/upload/complete"))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "asset_id": "bad", "filename": "x.bin", "content_type": "application/octet-stream",
            "created_at": "2026-08-11T18:00:00Z", "media_type": "video",
            "sha256": wrong_sha, "total_chunks": 1,
        }))
        .send().await.unwrap();
    assert!(!resp.status().is_success(), "hash mismatch must not succeed");
}

/// The chunk endpoints build a staging path from the client's `sha256`, so a
/// value carrying `..`, a separator or a drive letter must be refused outright —
/// otherwise any signed-in caller picks where uploaded bytes land on disk.
#[tokio::test]
async fn chunk_endpoints_reject_a_path_traversing_hash() {
    let (base, tmp) = spawn_server().await;
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    let escape_target = tmp.path().join("pwned");
    let hostile = [
        "../../pwned",
        r"..\..\pwned",
        &format!("{}", escape_target.to_string_lossy()),
        "not-hex-but-64-characters-long-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "abc",
    ];

    for sha in hostile {
        let meta = serde_json::json!({ "sha256": sha, "chunk_index": 0 });
        let form = reqwest::multipart::Form::new()
            .text("metadata", meta.to_string())
            .part("file", reqwest::multipart::Part::bytes(b"payload".to_vec()).file_name("part"));
        let resp = client.post(format!("{base}/media/upload/chunk")).bearer_auth(&token).multipart(form).send().await.unwrap();
        assert_eq!(resp.status(), 400, "chunk with sha256={sha:?} must be rejected");

        let status = client.get(format!("{base}/media/upload/status/{sha}")).bearer_auth(&token).send().await.unwrap();
        assert!(status.status() == 400 || status.status() == 404, "status with sha256={sha:?} got {}", status.status());

        let complete = serde_json::json!({
            "asset_id": "evil", "filename": "x.mp4", "content_type": "video/mp4",
            "created_at": "2026-08-11T18:00:00Z", "media_type": "video", "sha256": sha, "total_chunks": 1,
        });
        let done = client.post(format!("{base}/media/upload/complete")).bearer_auth(&token).json(&complete).send().await.unwrap();
        assert_eq!(done.status(), 400, "complete with sha256={sha:?} must be rejected");
    }

    assert!(!escape_target.exists(), "nothing may be written outside the chunk staging dir");
    assert!(!tmp.path().join("pwned").exists());
}

/// A chunked upload whose chunks are missing must not leave a half-assembled
/// temp file behind in the photo library.
#[tokio::test]
async fn failed_assembly_leaves_no_temp_file_in_the_library() {
    let (base, tmp) = spawn_server().await;
    let pictures = tmp.path().join("pictures");
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    // Declare four chunks but never upload any of them.
    let complete = serde_json::json!({
        "asset_id": "incomplete", "filename": "BIG.mp4", "content_type": "video/mp4",
        "created_at": "2026-08-11T18:00:00Z", "media_type": "video",
        "sha256": "b".repeat(64), "total_chunks": 4,
    });
    let resp = client.post(format!("{base}/media/upload/complete")).bearer_auth(&token).json(&complete).send().await.unwrap();
    assert_eq!(resp.status(), 500, "assembly of missing chunks fails");

    let folder = pictures.join("2026/202608-phone-sync");
    let leftovers: Vec<String> = std::fs::read_dir(&folder)
        .map(|entries| entries.map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "no partial file left behind, found {leftovers:?}");
}

#[tokio::test]
async fn client_can_upload_thumbnail_for_undecodable_media() {
    let (base, _tmp) = spawn_server().await;
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    // A "HEIC" the server can't decode/thumbnail on its own.
    let form = upload_form("heic-1", "IMG_0001.HEIC", "image/heic", "photo", b"pseudo-heic-bytes".to_vec());
    let up: Value = client.post(format!("{base}/media/upload")).bearer_auth(&token).multipart(form).send().await.unwrap().json().await.unwrap();
    let id = up["id"].as_str().unwrap().to_string();

    // No server-generatable thumbnail yet.
    let none = client.get(format!("{base}/media/{id}/thumb")).bearer_auth(&token).send().await.unwrap();
    assert_eq!(none.status(), 400);

    // Listing exposes the asset id and thumbnailable=false.
    let list: Value = client.get(format!("{base}/api/media")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    let item = list["items"].as_array().unwrap().iter().find(|i| i["id"] == id).unwrap();
    assert_eq!(item["asset_id"], "heic-1");
    // The server reports everything as thumbnailable (it will try ffmpeg); the
    // actual bytes only exist once generated or uploaded.
    assert_eq!(item["thumbnailable"], true);

    // Client uploads a preview; it is then served and the item becomes thumbnailable.
    let thumb = make_png();
    let put = client.post(format!("{base}/media/{id}/thumb")).bearer_auth(&token).body(thumb.clone()).send().await.unwrap();
    assert_eq!(put.status(), 201);

    let got = client.get(format!("{base}/media/{id}/thumb")).bearer_auth(&token).send().await.unwrap();
    assert_eq!(got.status(), 200);
    assert_eq!(got.bytes().await.unwrap().to_vec(), thumb);

    let list2: Value = client.get(format!("{base}/api/media")).bearer_auth(&token).send().await.unwrap().json().await.unwrap();
    let item2 = list2["items"].as_array().unwrap().iter().find(|i| i["id"] == id).unwrap();
    assert_eq!(item2["thumbnailable"], true);
}

#[tokio::test]
async fn verify_confirms_only_exact_full_items() {
    use sha2::{Digest, Sha256};
    let (base, tmp) = spawn_server().await;
    let client = reqwest::Client::new();
    let token = login(&base, &client).await;

    let bytes = b"the-only-copy-of-this-photo".to_vec();
    let size = bytes.len() as u64;
    let sha = hex::encode(Sha256::digest(&bytes));
    let form = upload_form("del-1", "IMG_9.jpg", "image/jpeg", "photo", bytes.clone());
    client.post(format!("{base}/media/upload")).bearer_auth(&token).multipart(form).send().await.unwrap();

    // A batch mixing a good item, a missing item, and a wrong size.
    let missing_sha = hex::encode(Sha256::digest(b"not uploaded"));
    let resp: Value = client
        .post(format!("{base}/media/verify"))
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "deep": true,
            "items": [
                { "sha256": sha, "size": size },
                { "sha256": missing_sha, "size": 10 },
                { "sha256": sha, "size": size + 1 },
            ]
        }))
        .send().await.unwrap().json().await.unwrap();

    let results = resp["results"].as_array().unwrap();
    let by_reason = |i: usize| results[i]["reason"].as_str().unwrap();
    assert_eq!(results[0]["verified"], true);
    assert_eq!(by_reason(0), "ok");
    assert_eq!(results[1]["verified"], false);
    assert_eq!(by_reason(1), "not_found");
    assert_eq!(results[2]["verified"], false);
    assert_eq!(by_reason(2), "size_mismatch");

    // Deep verification catches on-disk corruption that size/index checks miss.
    let stored = find_stored_file(&tmp);
    std::fs::write(&stored, b"the-only-copy-of-this-photX").unwrap(); // same length, different bytes
    let corrupt: Value = client
        .post(format!("{base}/media/verify"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "deep": true, "items": [{ "sha256": sha, "size": size }] }))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(corrupt["results"][0]["verified"], false);
    assert_eq!(corrupt["results"][0]["reason"], "hash_mismatch");
}

/// Finds the single stored media file under the temp server's pictures tree.
fn find_stored_file(tmp: &tempfile::TempDir) -> std::path::PathBuf {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() { walk(&path, out); }
            else if path.extension().map(|e| e == "jpg").unwrap_or(false) { out.push(path); }
        }
    }
    let mut files = Vec::new();
    walk(&tmp.path().join("pictures"), &mut files);
    files.into_iter().next().expect("a stored jpg")
}
