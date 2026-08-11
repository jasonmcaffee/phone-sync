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

// MARK: - Chunked upload

/// Tunables for how large files are chunked. Cloudflare's proxied edge caps a
/// single request body at 100 MB, so anything larger is uploaded in pieces.
enum SyncTuning {
    /// Max bytes per chunk (90 MB, safely under the 100 MB edge limit). In DEBUG
    /// builds a `CHUNK_SIZE_OVERRIDE` env var can shrink this so the chunked
    /// path can be exercised without a multi-GB file.
    static var chunkSize: Int {
        #if DEBUG
        if let raw = ProcessInfo.processInfo.environment["CHUNK_SIZE_OVERRIDE"], let value = Int(raw) {
            return value
        }
        #endif
        return 90 * 1000 * 1000
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
