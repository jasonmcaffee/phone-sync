import XCTest
@testable import PhoneSync

/// Unit tests for the pure sync-diff logic that decides what to upload and how
/// to reconcile local state against the server manifest.
final class SyncDiffTests: XCTestCase {

    /// Assets already in the manifest are excluded from the upload set.
    func testNotSyncedExcludesManifestItems() {
        let all = ["a", "b", "c"]
        let manifest: Set<String> = ["b"]
        let result = SyncDiff.notSynced(allAssetIds: all, manifest: manifest, records: [:])
        XCTAssertEqual(result, ["a", "c"])
    }

    /// Assets marked synced locally are excluded even if absent from the manifest.
    func testNotSyncedExcludesLocallySynced() {
        let all = ["a", "b"]
        let records = ["a": StoredSyncRecord(state: .synced, sha256: nil, serverId: nil, lastAttempt: nil)]
        let result = SyncDiff.notSynced(allAssetIds: all, manifest: [], records: records)
        XCTAssertEqual(result, ["b"])
    }

    /// Failed items are retried (remain in the not-synced set).
    func testFailedItemsAreRetried() {
        let all = ["a"]
        let records = ["a": StoredSyncRecord(state: .failed, sha256: nil, serverId: nil, lastAttempt: nil)]
        let result = SyncDiff.notSynced(allAssetIds: all, manifest: [], records: records)
        XCTAssertEqual(result, ["a"])
    }

    /// Reconcile marks manifest-present assets as synced without touching others.
    func testReconcileMarksManifestItemsSynced() {
        let all = ["a", "b", "c"]
        let manifest: Set<String> = ["a", "c"]
        let updated = SyncDiff.reconcile(allAssetIds: all, manifest: manifest, records: [:])
        XCTAssertEqual(updated["a"]?.state, .synced)
        XCTAssertEqual(updated["c"]?.state, .synced)
        XCTAssertNil(updated["b"])
    }

    /// A full round-trip: reconcile then diff yields only the truly missing item.
    func testReconcileThenDiff() {
        let all = ["a", "b", "c"]
        let manifest: Set<String> = ["a"]
        let reconciled = SyncDiff.reconcile(allAssetIds: all, manifest: manifest, records: [:])
        let todo = SyncDiff.notSynced(allAssetIds: all, manifest: manifest, records: reconciled)
        XCTAssertEqual(todo, ["b", "c"])
    }
}
