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
        username: "jason".into(),
        password_hash: auth::hash_password("modestMouse1!"),
        jwt_secret: "test-secret".into(),
        token_ttl_secs: 365 * 24 * 60 * 60,
        max_upload_bytes: 10 * 1024 * 1024,
    };
    let storage = Storage::open(config.data_dir.clone()).unwrap();
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
    use sha2::{Digest, Sha256};
    let sha = hex::encode(Sha256::digest(&bytes));
    let meta = serde_json::json!({
        "asset_id": asset_id,
        "filename": filename,
        "content_type": content_type,
        "created_at": "2026-08-11T00:00:00Z",
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
