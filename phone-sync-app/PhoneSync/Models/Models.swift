import Foundation

/// Where the backend lives. Editable at runtime (LAN IP for dev, domain for prod).
struct ServerConfig: Equatable {
    var baseURL: String

    /// Default development server. Replace via Settings, or point at
    /// https://phone.jasonmcaffee.com in production.
    static let devDefault = ServerConfig(baseURL: "http://192.168.0.26:8080")
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
