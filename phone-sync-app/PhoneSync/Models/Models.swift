import Foundation

/// Where the backend lives. Editable at runtime (LAN IP for dev, domain for prod).
struct ServerConfig: Equatable {
    var baseURL: String

    /// Default server the app points at on first launch. Editable via the
    /// sign-in screen and Settings (e.g. a LAN `http://<ip>:8080` for local dev).
    static let defaultServer = ServerConfig(baseURL: "https://phone.jasonmcaffee.com")
}

/// A signed-in session's bearer token and its expiry.
struct AuthToken: Codable, Equatable {
    let token: String
    let expiresAt: Int   // unix seconds

    /// True while the token is still within its validity window.
    var isValid: Bool {
        Double(expiresAt) > Date().timeIntervalSince1970
    }
}

/// Per-asset backup status shown on each grid cell.
enum SyncState: String, Codable, Equatable {
    case notSynced
    case syncing
    case synced
    case failed
}

// MARK: - API DTOs

/// Sign-in request body.
struct LoginRequest: Encodable {
    let username: String
    let password: String
}

/// Sign-in response with the long-lived token.
struct LoginResponse: Decodable {
    let token: String
    let expiresAt: Int

    enum CodingKeys: String, CodingKey {
        case token
        case expiresAt = "expires_at"
    }
}

/// Server's list of already-stored asset ids.
struct ManifestResponse: Decodable {
    let assetIds: [String]
    let count: Int

    enum CodingKeys: String, CodingKey {
        case assetIds = "asset_ids"
        case count
    }
}

/// Metadata sent alongside an upload (JSON multipart part).
struct UploadMetadata: Encodable {
    let assetId: String
    let filename: String
    let contentType: String
    let createdAt: String
    let mediaType: String
    let sha256: String

    enum CodingKeys: String, CodingKey {
        case assetId = "asset_id"
        case filename
        case contentType = "content_type"
        case createdAt = "created_at"
        case mediaType = "media_type"
        case sha256
    }
}

/// Server response to an upload.
struct UploadResponse: Decodable {
    let id: String
    let sha256: String
    let stored: Bool
    let duplicate: Bool
}

/// Locally persisted record of an asset's sync progress.
struct StoredSyncRecord: Codable {
    var state: SyncState
    var sha256: String?
    var serverId: String?
    var lastAttempt: Double?
}

// MARK: - Server library listing (for the "Synced" view)

/// A media item as returned by the server's gallery listing (`/api/media`).
/// The Synced view shows these (what the server actually holds) rather than
/// local assets, since local synced items may be deleted later.
struct MediaListItem: Decodable {
    let id: String            // sha256 content id
    let assetId: String       // client localIdentifier (maps back to a local asset)
    let filename: String
    let contentType: String
    let mediaType: String     // "photo" | "video"
    let createdAt: String     // ISO-8601
    let size: Int64
    let thumbnailable: Bool

    enum CodingKeys: String, CodingKey {
        case id, filename, size, thumbnailable
        case assetId = "asset_id"
        case contentType = "content_type"
        case mediaType = "media_type"
        case createdAt = "created_at"
    }
}

/// The server gallery listing response.
struct MediaListResponse: Decodable {
    let items: [MediaListItem]
    let count: Int
}

// MARK: - Chunked upload

/// Tunables for how large files are chunked. Cloudflare's proxied edge caps a
/// single request body at 100 MB, so anything larger is uploaded in pieces.
enum SyncTuning {
    /// Max bytes per chunk. Kept modest (20 MB) so each request's body stays
    /// small in memory — well under Cloudflare's 100 MB cap and, combined with
    /// streaming upload bodies from disk, avoiding the memory blow-up that was
    /// OOM-killing the app on large videos. In DEBUG builds a `CHUNK_SIZE_OVERRIDE`
    /// env var can shrink this further to exercise the chunked path in tests.
    static var chunkSize: Int {
        #if DEBUG
        if let raw = ProcessInfo.processInfo.environment["CHUNK_SIZE_OVERRIDE"], let value = Int(raw) {
            return value
        }
        #endif
        return 20 * 1024 * 1024
    }
}

/// Server's view of an in-progress chunked upload: whether the full content is
/// already stored, and which chunk indices it already holds (for resume).
struct ChunkStatusResponse: Decodable {
    let stored: Bool
    let received: [Int]
}

/// Acknowledgement that one chunk was stored.
struct ChunkAck: Decodable {
    let received: Int
    let ok: Bool
}

/// Finalize request that tells the server to assemble the uploaded chunks.
struct CompleteRequest: Encodable {
    let assetId: String
    let filename: String
    let contentType: String
    let createdAt: String
    let mediaType: String
    let sha256: String
    let totalChunks: Int

    enum CodingKeys: String, CodingKey {
        case assetId = "asset_id"
        case filename
        case contentType = "content_type"
        case createdAt = "created_at"
        case mediaType = "media_type"
        case sha256
        case totalChunks = "total_chunks"
    }
}
