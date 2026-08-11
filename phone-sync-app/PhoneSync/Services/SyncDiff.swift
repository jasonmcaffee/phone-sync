import Foundation

/// Pure sync-diff logic, separated from PhotoKit so it can be unit tested.
enum SyncDiff {
    /// Given all local asset ids, the server's manifest set, and local records,
    /// returns the ids that still need uploading: those not in the manifest and
    /// not already marked synced locally.
    static func notSynced(allAssetIds: [String], manifest: Set<String>, records: [String: StoredSyncRecord]) -> [String] {
        allAssetIds.filter { id in
            if manifest.contains(id) { return false }
            if records[id]?.state == .synced { return false }
            return true
        }
    }

    /// Reconciles local records against the server manifest: any asset present
    /// on the server is marked `.synced` locally. Returns the updated records.
    static func reconcile(allAssetIds: [String], manifest: Set<String>, records: [String: StoredSyncRecord]) -> [String: StoredSyncRecord] {
        var updated = records
        for id in allAssetIds where manifest.contains(id) {
            var record = updated[id] ?? StoredSyncRecord(state: .notSynced, sha256: nil, serverId: nil, lastAttempt: nil)
            record.state = .synced
            updated[id] = record
        }
        return updated
    }
}
