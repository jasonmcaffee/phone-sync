import Foundation
import Photos

/// Deletes local copies of backed-up items — but only after the server confirms,
/// per item, that it holds the exact full file (content hash + size, with a
/// server-side on-disk re-hash). Anything not confirmed is kept. iOS also shows
/// its own delete confirmation before anything is removed.
@MainActor
final class DeletionEngine: ObservableObject {
    /// A local item that is a candidate for deletion.
    struct Candidate: Equatable {
        let assetId: String
        let sha256: String
        let size: Int64
    }

    /// Outcome of a deletion run.
    struct Result {
        let deletedCount: Int
        let freedBytes: Int64
        /// assetId -> reason it was kept (not verified / network error).
        let kept: [(assetId: String, reason: String)]
        /// True if the user cancelled the iOS delete prompt.
        let cancelled: Bool
    }

    @Published private(set) var isWorking = false
    /// Progress of the verify phase: (checked, total).
    @Published private(set) var progress: (checked: Int, total: Int) = (0, 0)
    @Published private(set) var lastResult: Result?

    private let photoService: PhotoLibraryService
    private let api: ApiClient
    private let auth: AuthService
    private let syncEngine: SyncEngine

    /// Batch verify by cumulative bytes so each request's server-side deep re-hash
    /// stays well under the edge timeout, even with large videos.
    private let maxBatchBytes: Int64 = 2_000_000_000
    private let maxBatchCount = 100

    init(photoService: PhotoLibraryService, api: ApiClient, auth: AuthService, syncEngine: SyncEngine) {
        self.photoService = photoService
        self.api = api
        self.auth = auth
        self.syncEngine = syncEngine
    }

    /// Builds delete candidates from the **server's** listing (authoritative
    /// sha256 + size and the asset id), intersected with assets still on this
    /// device. This covers everything the server holds, regardless of local
    /// record state. Returns [] if the listing can't be fetched (fail-safe).
    func gatherCandidates(token: String) async -> [Candidate] {
        guard let items = try? await api.fetchMediaList(token: token) else { return [] }
        let onDevice = photoService.existingLocalIdentifiers(items.map { $0.assetId })
        return items.compactMap { item in
            guard onDevice.contains(item.assetId) else { return nil }
            return Candidate(assetId: item.assetId, sha256: item.id, size: item.size)
        }
    }

    /// Total reclaimable space: how many candidates and how many bytes.
    func reclaimable() async -> (count: Int, bytes: Int64) {
        guard let token = auth.bearer else { return (0, 0) }
        let candidates = await gatherCandidates(token: token)
        return (candidates.count, candidates.reduce(0) { $0 + $1.size })
    }

    /// Verifies all candidates against the server and deletes the confirmed ones.
    func deleteAllBackedUp() async {
        guard !isWorking, let token = auth.bearer else { return }
        isWorking = true
        defer { isWorking = false }

        let candidates = await gatherCandidates(token: token)
        progress = (0, candidates.count)
        guard !candidates.isEmpty else {
            lastResult = Result(deletedCount: 0, freedBytes: 0, kept: [], cancelled: false)
            return
        }

        var verified: [Candidate] = []
        var kept: [(assetId: String, reason: String)] = []

        for batch in batched(candidates) {
            do {
                let items = batch.map { VerifyRequestItem(sha256: $0.sha256, size: $0.size) }
                let results = try await api.verify(items: items, deep: true, token: token)
                let plan = Self.plan(candidates: batch, results: results)
                verified += plan.delete
                kept += plan.kept
            } catch {
                // Any failure keeps the whole batch — never delete on uncertainty.
                kept += batch.map { ($0.assetId, "network_error") }
            }
            progress = (progress.checked + batch.count, candidates.count)
        }

        guard !verified.isEmpty else {
            lastResult = Result(deletedCount: 0, freedBytes: 0, kept: kept, cancelled: false)
            return
        }

        do {
            try await photoService.deleteAssets(withLocalIdentifiers: verified.map { $0.assetId })
            syncEngine.forget(assetIds: verified.map { $0.assetId })
            let freed = verified.reduce(Int64(0)) { $0 + $1.size }
            lastResult = Result(deletedCount: verified.count, freedBytes: freed, kept: kept, cancelled: false)
        } catch {
            // User cancelled the iOS prompt, or the change failed: nothing deleted.
            lastResult = Result(deletedCount: 0, freedBytes: 0, kept: kept, cancelled: true)
        }
    }

    /// Pure decision: given a batch and the server's verify results, returns which
    /// candidates to delete and which to keep (with reasons). A candidate is
    /// deletable only if the server returned `verified == true` for its hash.
    nonisolated static func plan(candidates: [Candidate], results: [VerifyResult]) -> (delete: [Candidate], kept: [(assetId: String, reason: String)]) {
        let verifiedHashes = Set(results.filter { $0.verified }.map { $0.sha256 })
        let reasonByHash = Dictionary(results.map { ($0.sha256, $0.reason) }, uniquingKeysWith: { first, _ in first })
        var delete: [Candidate] = []
        var kept: [(assetId: String, reason: String)] = []
        for candidate in candidates {
            if verifiedHashes.contains(candidate.sha256) {
                delete.append(candidate)
            } else {
                kept.append((candidate.assetId, reasonByHash[candidate.sha256] ?? "unverified"))
            }
        }
        return (delete, kept)
    }

    /// Splits candidates into batches bounded by cumulative bytes and count.
    private func batched(_ candidates: [Candidate]) -> [[Candidate]] {
        var batches: [[Candidate]] = []
        var current: [Candidate] = []
        var bytes: Int64 = 0
        for candidate in candidates {
            if !current.isEmpty && (bytes + candidate.size > maxBatchBytes || current.count >= maxBatchCount) {
                batches.append(current)
                current = []
                bytes = 0
            }
            current.append(candidate)
            bytes += candidate.size
        }
        if !current.isEmpty { batches.append(current) }
        return batches
    }
}
