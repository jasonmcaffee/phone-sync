import Foundation
import Photos
import CryptoKit

/// Coordinates backing up local media to the server. Owns per-asset sync
/// records (published for the UI), reconciles against the server manifest, and
/// uploads the not-yet-synced set. Triggered manually, by library changes, or
/// by a background task.
@MainActor
final class SyncEngine: ObservableObject {
    /// asset localIdentifier -> current sync record. Drives grid badges.
    @Published private(set) var records: [String: StoredSyncRecord] = [:]
    /// True while a sync pass is running.
    @Published private(set) var isSyncing = false
    /// Human-readable last error, if any.
    @Published var lastError: String?
    /// Progress of the current/last run.
    @Published private(set) var uploadedThisRun = 0
    @Published private(set) var totalThisRun = 0

    private let photoService: PhotoLibraryService
    private let api: ApiClient
    private let auth: AuthService
    private let store: SyncStateStore

    /// Wires dependencies and loads persisted sync records.
    init(photoService: PhotoLibraryService, api: ApiClient, auth: AuthService, store: SyncStateStore) {
        self.photoService = photoService
        self.api = api
        self.auth = auth
        self.store = store
        self.records = store.load()
    }

    /// Returns the current sync state for an asset (defaults to notSynced).
    func state(for assetId: String) -> SyncState {
        records[assetId]?.state ?? .notSynced
    }

    /// Count of assets currently marked synced.
    var syncedCount: Int {
        records.values.filter { $0.state == .synced }.count
    }

    /// Fetches the server manifest and marks matching local assets as synced.
    /// Safe to call on launch/sign-in before any upload.
    func reconcile(assets: [PHAsset]) async {
        guard let token = auth.bearer else { return }
        do {
            let manifest = try await api.fetchManifest(token: token)
            let ids = assets.map { $0.localIdentifier }
            records = SyncDiff.reconcile(allAssetIds: ids, manifest: manifest, records: records)
            store.save(records)
        } catch let error as ApiError {
            handle(error)
        } catch {
            lastError = error.localizedDescription
        }
    }

    /// Uploads every not-yet-synced asset. Reconciles first so we never
    /// re-upload what the server already has. Updates badges as it goes.
    func syncAll(assets: [PHAsset]) async {
        guard !isSyncing else { return }
        guard let token = auth.bearer else {
            lastError = "Not signed in."
            return
        }
        isSyncing = true
        lastError = nil
        defer { isSyncing = false }

        await reconcile(assets: assets)

        let manifest = Set(records.filter { $0.value.state == .synced }.keys)
        let byId = Dictionary(uniqueKeysWithValues: assets.map { ($0.localIdentifier, $0) })
        let todo = SyncDiff.notSynced(allAssetIds: assets.map { $0.localIdentifier }, manifest: manifest, records: records)

        totalThisRun = todo.count
        uploadedThisRun = 0

        for assetId in todo {
            guard let asset = byId[assetId] else { continue }
            await uploadOne(asset: asset, token: token)
            if auth.bearer == nil { break } // signed out due to 401
        }
    }

    /// Uploads a single asset, updating its record through syncing/synced/failed.
    private func uploadOne(asset: PHAsset, token: String) async {
        let assetId = asset.localIdentifier
        setState(.syncing, for: assetId)

        guard let exported = await photoService.exportForUpload(asset) else {
            setState(.failed, for: assetId)
            lastError = "Could not read asset \(assetId)."
            return
        }

        let sha = hexSHA256(exported.data)
        let metadata = UploadMetadata(
            assetId: assetId,
            filename: exported.filename,
            contentType: exported.contentType,
            createdAt: iso8601(asset.creationDate),
            mediaType: exported.mediaType,
            sha256: sha
        )

        do {
            let response = try await api.upload(metadata: metadata, fileData: exported.data, token: token)
            var record = records[assetId] ?? StoredSyncRecord(state: .synced, sha256: sha, serverId: nil, lastAttempt: nil)
            record.state = .synced
            record.sha256 = sha
            record.serverId = response.id
            record.lastAttempt = Date().timeIntervalSince1970
            records[assetId] = record
            store.save(records)
            uploadedThisRun += 1
        } catch let error as ApiError {
            setState(.failed, for: assetId)
            handle(error)
        } catch {
            setState(.failed, for: assetId)
            lastError = error.localizedDescription
        }
    }

    /// Updates and persists a single asset's state.
    private func setState(_ state: SyncState, for assetId: String) {
        var record = records[assetId] ?? StoredSyncRecord(state: state, sha256: nil, serverId: nil, lastAttempt: nil)
        record.state = state
        record.lastAttempt = Date().timeIntervalSince1970
        records[assetId] = record
        store.save(records)
    }

    /// Reacts to API errors — a 401 forces sign-out so the UI shows sign-in.
    private func handle(_ error: ApiError) {
        if case .unauthorized = error {
            auth.signOut()
        }
        lastError = error.localizedDescription
    }

    /// Hex-encodes the SHA-256 of the given bytes.
    private func hexSHA256(_ data: Data) -> String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }

    /// Formats a date as ISO-8601, or empty string if nil.
    private func iso8601(_ date: Date?) -> String {
        guard let date = date else { return "" }
        return ISO8601DateFormatter().string(from: date)
    }
}
