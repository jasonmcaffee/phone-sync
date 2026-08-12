import Foundation
import Photos
import CryptoKit
import os

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
    /// Smoothed upload throughput in MB/s (0 when idle).
    @Published private(set) var uploadSpeedMBps: Double = 0

    private let photoService: PhotoLibraryService
    private let api: ApiClient
    private let auth: AuthService
    private let store: SyncStateStore
    private let log = Logger(subsystem: "com.jasonmcaffee.phonesync", category: "sync")

    /// Trips after this many *consecutive* transport/server failures — i.e. the
    /// server looks down — rather than on a single bad item, so one problematic
    /// file can't block the rest of the library.
    private let failureThreshold = 3
    /// Trips on repeated transport/server errors to pace retries during an outage.
    private let breaker = CircuitBreaker()
    /// The pending backoff retry, if any.
    private var retryTask: Task<Void, Never>?
    /// Wall-clock start of the current run, for throughput display.
    private var runStart: Date?

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
        uploadSpeedMBps = 0
        runStart = Date()
        retryTask?.cancel()
        defer { isSyncing = false; uploadSpeedMBps = 0 }

        await reconcile(assets: assets)

        // Only attempt items that are neither on the server nor already synced.
        let manifest = Set(records.filter { $0.value.state == .synced }.keys)
        let byId = Dictionary(uniqueKeysWithValues: assets.map { ($0.localIdentifier, $0) })
        let todo = SyncDiff.notSynced(allAssetIds: assets.map { $0.localIdentifier }, manifest: manifest, records: records)

        totalThisRun = todo.count
        uploadedThisRun = 0
        log.notice("sync start: \(todo.count, privacy: .public) item(s) to upload")

        var consecutiveFailures = 0
        for assetId in todo {
            guard let asset = byId[assetId] else { continue }
            switch await uploadOne(asset: asset, token: token) {
            case .success, .assetError:
                // Success, or a single unreadable/oversized item we skip past.
                consecutiveFailures = 0
            case .authExpired:
                return // signed out; the UI shows sign-in
            case .transportFailure:
                consecutiveFailures += 1
                if consecutiveFailures >= failureThreshold {
                    // Repeated failures: the server looks down. Trip the breaker,
                    // stop the batch, and retry later with backoff.
                    breaker.recordFailure()
                    log.error("circuit breaker tripped after \(self.failureThreshold, privacy: .public) consecutive failures")
                    scheduleBackoffRetry()
                    return
                }
                // Otherwise skip this item and continue with the next.
            }
        }

        breaker.reset()
        log.notice("sync finished: \(self.uploadedThisRun, privacy: .public)/\(self.totalThisRun, privacy: .public) uploaded")
    }

    /// Uploads a single asset, updating its record through syncing/synced/failed,
    /// and returns the outcome so the caller can drive the circuit breaker. The
    /// asset is exported to a temp file (never buffered whole in memory); files
    /// larger than the edge limit are uploaded in resumable chunks read from disk.
    private func uploadOne(asset: PHAsset, token: String) async -> UploadOutcome {
        let assetId = asset.localIdentifier
        setState(.syncing, for: assetId)

        guard let file = await photoService.exportToTempFile(asset) else {
            setState(.failed, for: assetId)
            lastError = "Could not read asset \(assetId)."
            return .assetError
        }
        defer { try? FileManager.default.removeItem(at: file.url) }

        guard let sha = sha256OfFile(file.url) else {
            setState(.failed, for: assetId)
            lastError = "Could not hash \(file.filename)."
            return .assetError
        }
        let sizeMB = Double(file.size) / 1_000_000
        let availMB = Int(os_proc_available_memory()) / 1_000_000
        log.notice("uploading \(file.filename, privacy: .public) \(sizeMB, format: .fixed(precision: 1), privacy: .public) MB \(file.size > SyncTuning.chunkSize ? "chunked" : "single", privacy: .public) memAvail=\(availMB, privacy: .public)MB")

        do {
            let response: UploadResponse
            if file.size > SyncTuning.chunkSize {
                response = try await uploadChunked(assetId: assetId, file: file, sha: sha, createdAt: iso8601(asset.creationDate), token: token)
            } else {
                let data = try Data(contentsOf: file.url)
                let metadata = UploadMetadata(assetId: assetId, filename: file.filename, contentType: file.contentType, createdAt: iso8601(asset.creationDate), mediaType: file.mediaType, sha256: sha)
                let started = Date()
                response = try await api.upload(metadata: metadata, fileData: data, token: token)
                recordThroughput(bytes: data.count, seconds: Date().timeIntervalSince(started))
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
            log.error("upload failed for \(file.filename, privacy: .public): \(error.localizedDescription, privacy: .public)")
            return .transportFailure
        } catch {
            setState(.failed, for: assetId)
            lastError = error.localizedDescription
            log.error("upload failed for \(file.filename, privacy: .public): \(error.localizedDescription, privacy: .public)")
            return .transportFailure
        }
    }

    /// Uploads a large file in ≤90 MB chunks read one at a time from disk (so the
    /// whole video is never in memory), resuming from whatever the server already
    /// has, then asks the server to assemble and verify them.
    private func uploadChunked(assetId: String, file: ExportedFile, sha: String, createdAt: String, token: String) async throws -> UploadResponse {
        let total = (file.size + SyncTuning.chunkSize - 1) / SyncTuning.chunkSize

        // Skip anything the server already stores or already received.
        let status = try await api.uploadStatus(sha256: sha, token: token)
        if status.stored {
            return UploadResponse(id: sha, sha256: sha, stored: true, duplicate: true)
        }
        let have = Set(status.received)

        let handle = try FileHandle(forReadingFrom: file.url)
        defer { try? handle.close() }
        for index in 0..<total where !have.contains(index) {
            try handle.seek(toOffset: UInt64(index * SyncTuning.chunkSize))
            let chunk = handle.readData(ofLength: SyncTuning.chunkSize)
            let started = Date()
            try await api.uploadChunk(sha256: sha, chunkIndex: index, chunkData: chunk, token: token)
            recordThroughput(bytes: chunk.count, seconds: Date().timeIntervalSince(started))
            log.debug("chunk \(index + 1, privacy: .public)/\(total, privacy: .public) sent for \(file.filename, privacy: .public)")
        }

        let complete = CompleteRequest(assetId: assetId, filename: file.filename, contentType: file.contentType, createdAt: createdAt, mediaType: file.mediaType, sha256: sha, totalChunks: total)
        return try await api.uploadComplete(complete, token: token)
    }

    /// Streams a file through SHA-256 in blocks, so hashing a large video doesn't
    /// require loading it into memory. Each block is read inside an
    /// `autoreleasepool` because `FileHandle.readData` returns autoreleased
    /// buffers that would otherwise accumulate (the whole file!) in this tight
    /// synchronous loop and OOM-kill the app. Returns nil if the file can't be read.
    private func sha256OfFile(_ url: URL) -> String? {
        guard let handle = try? FileHandle(forReadingFrom: url) else { return nil }
        defer { try? handle.close() }
        var hasher = SHA256()
        var reachedEnd = false
        while !reachedEnd {
            autoreleasepool {
                let block = handle.readData(ofLength: 4 * 1024 * 1024)
                if block.isEmpty {
                    reachedEnd = true
                } else {
                    hasher.update(data: block)
                }
            }
        }
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }

    /// Updates the smoothed upload-speed reading (exponential moving average).
    private func recordThroughput(bytes: Int, seconds: Double) {
        guard seconds > 0 else { return }
        let instant = Double(bytes) / seconds / 1_000_000 // MB/s
        uploadSpeedMBps = uploadSpeedMBps == 0 ? instant : uploadSpeedMBps * 0.6 + instant * 0.4
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
