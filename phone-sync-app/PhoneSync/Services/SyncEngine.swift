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

    /// Trips on transport/server errors to stop a failing batch and pace retries.
    private let breaker = CircuitBreaker()
    /// The pending backoff retry, if any.
    private var retryTask: Task<Void, Never>?

    /// Outcome of a single asset upload, used to drive the circuit breaker.
    private enum UploadOutcome {
        case success
        /// The asset itself couldn't be read/exported; skip it, keep going.
        case assetError
        /// A network/server failure; trips the breaker and stops the batch.
        case transportFailure
        /// The session expired (401); stop and show sign-in.
        case authExpired
    }

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
    /// re-upload what the server already has, then uploads only the items that
    /// are still missing. A transport/server error trips the circuit breaker:
    /// the batch stops immediately and a backoff retry is scheduled.
    func syncAll(assets: [PHAsset]) async {
        guard !isSyncing else { return }
        guard let token = auth.bearer else {
            lastError = "Not signed in."
            return
        }
        isSyncing = true
        lastError = nil
        retryTask?.cancel()
        defer { isSyncing = false }

        await reconcile(assets: assets)

        // Only attempt items that are neither on the server nor already synced.
        let manifest = Set(records.filter { $0.value.state == .synced }.keys)
        let byId = Dictionary(uniqueKeysWithValues: assets.map { ($0.localIdentifier, $0) })
        let todo = SyncDiff.notSynced(allAssetIds: assets.map { $0.localIdentifier }, manifest: manifest, records: records)

        totalThisRun = todo.count
        uploadedThisRun = 0

        var tripped = false
        for assetId in todo {
            guard let asset = byId[assetId] else { continue }
            let outcome = await uploadOne(asset: asset, token: token)
            switch outcome {
            case .success, .assetError:
                continue
            case .authExpired:
                return // signed out; the UI will show sign-in
            case .transportFailure:
                // Circuit breaker: stop processing the rest of this batch.
                tripped = true
            }
            if tripped { break }
        }

        if tripped {
            breaker.recordFailure()
            scheduleBackoffRetry()
        } else {
            breaker.reset()
        }
    }

    /// Uploads a single asset, updating its record through syncing/synced/failed,
    /// and returns the outcome so the caller can drive the circuit breaker.
    /// Files larger than the edge body limit are uploaded in resumable chunks.
    private func uploadOne(asset: PHAsset, token: String) async -> UploadOutcome {
        let assetId = asset.localIdentifier
        setState(.syncing, for: assetId)

        guard let exported = await photoService.exportForUpload(asset) else {
            setState(.failed, for: assetId)
            lastError = "Could not read asset \(assetId)."
            return .assetError
        }

        let sha = hexSHA256(exported.data)
        do {
            let response: UploadResponse
            if exported.data.count > SyncTuning.chunkSize {
                response = try await uploadChunked(assetId: assetId, exported: exported, sha: sha, createdAt: iso8601(asset.creationDate), token: token)
            } else {
                let metadata = UploadMetadata(assetId: assetId, filename: exported.filename, contentType: exported.contentType, createdAt: iso8601(asset.creationDate), mediaType: exported.mediaType, sha256: sha)
                response = try await api.upload(metadata: metadata, fileData: exported.data, token: token)
            }
            markSynced(assetId: assetId, sha: sha, serverId: response.id)
            uploadedThisRun += 1
            return .success
        } catch let error as ApiError {
            setState(.failed, for: assetId)
            if case .unauthorized = error {
                auth.signOut()
                lastError = error.localizedDescription
                return .authExpired
            }
            lastError = error.localizedDescription
            return .transportFailure
        } catch {
            setState(.failed, for: assetId)
            lastError = error.localizedDescription
            return .transportFailure
        }
    }

    /// Uploads a large file in ≤90 MB chunks, resuming from whatever the server
    /// already has, then asks the server to assemble and verify them.
    private func uploadChunked(assetId: String, exported: ExportedAsset, sha: String, createdAt: String, token: String) async throws -> UploadResponse {
        let data = exported.data
        let total = (data.count + SyncTuning.chunkSize - 1) / SyncTuning.chunkSize

        // Skip anything the server already stores or already received.
        let status = try await api.uploadStatus(sha256: sha, token: token)
        if status.stored {
            return UploadResponse(id: sha, sha256: sha, stored: true, duplicate: true)
        }
        let have = Set(status.received)
        for index in 0..<total where !have.contains(index) {
            let start = index * SyncTuning.chunkSize
            let end = min(start + SyncTuning.chunkSize, data.count)
            let chunk = data.subdata(in: start..<end)
            try await api.uploadChunk(sha256: sha, chunkIndex: index, chunkData: chunk, token: token)
        }

        let complete = CompleteRequest(assetId: assetId, filename: exported.filename, contentType: exported.contentType, createdAt: createdAt, mediaType: exported.mediaType, sha256: sha, totalChunks: total)
        return try await api.uploadComplete(complete, token: token)
    }

    /// Marks an asset as synced and persists the record.
    private func markSynced(assetId: String, sha: String, serverId: String) {
        var record = records[assetId] ?? StoredSyncRecord(state: .synced, sha256: sha, serverId: nil, lastAttempt: nil)
        record.state = .synced
        record.sha256 = sha
        record.serverId = serverId
        record.lastAttempt = Date().timeIntervalSince1970
        records[assetId] = record
        store.save(records)
    }

    /// Schedules a backoff retry after the breaker trips, unless the retry
    /// budget is exhausted (then we wait for the next manual/library trigger).
    private func scheduleBackoffRetry() {
        guard breaker.canRetry else {
            lastError = "Sync paused after repeated errors. It will retry on the next change or manual sync."
            return
        }
        let delay = breaker.nextDelay()
        lastError = "Sync error — retrying in \(Int(delay))s."
        retryTask?.cancel()
        retryTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
            guard let self, !Task.isCancelled else { return }
            let assets = self.photoService.fetchAssets()
            await self.syncAll(assets: assets)
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
