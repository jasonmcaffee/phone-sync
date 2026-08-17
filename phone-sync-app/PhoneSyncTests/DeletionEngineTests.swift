import XCTest
@testable import PhoneSync

/// Tests the safety-critical decision: an item is only ever deletable when the
/// server explicitly verified its hash. Anything else is kept.
final class DeletionEngineTests: XCTestCase {

    private func candidate(_ id: String, _ sha: String, _ size: Int64 = 100) -> DeletionEngine.Candidate {
        DeletionEngine.Candidate(assetId: id, sha256: sha, size: size)
    }

    private func result(_ sha: String, _ verified: Bool, _ reason: String) -> VerifyResult {
        VerifyResult(sha256: sha, verified: verified, reason: reason)
    }

    /// Only verified hashes are deleted; unverified are kept with their reason.
    func testOnlyVerifiedAreDeleted() {
        let candidates = [candidate("a", "sha-a"), candidate("b", "sha-b"), candidate("c", "sha-c")]
        let results = [
            result("sha-a", true, "ok"),
            result("sha-b", false, "hash_mismatch"),
            result("sha-c", true, "ok"),
        ]
        let plan = DeletionEngine.plan(candidates: candidates, results: results)
        XCTAssertEqual(plan.delete.map { $0.assetId }.sorted(), ["a", "c"])
        XCTAssertEqual(plan.kept.count, 1)
        XCTAssertEqual(plan.kept.first?.assetId, "b")
        XCTAssertEqual(plan.kept.first?.reason, "hash_mismatch")
    }

    /// An item the server didn't return a result for is kept (never assumed OK).
    func testItemAbsentFromResultsIsKept() {
        let plan = DeletionEngine.plan(candidates: [candidate("a", "sha-a")], results: [])
        XCTAssertTrue(plan.delete.isEmpty)
        XCTAssertEqual(plan.kept.first?.assetId, "a")
        XCTAssertEqual(plan.kept.first?.reason, "unverified")
    }

    /// When nothing verifies, nothing is deleted.
    func testNothingVerifiedDeletesNothing() {
        let candidates = [candidate("a", "sha-a"), candidate("b", "sha-b")]
        let results = [result("sha-a", false, "not_found"), result("sha-b", false, "size_mismatch")]
        let plan = DeletionEngine.plan(candidates: candidates, results: results)
        XCTAssertTrue(plan.delete.isEmpty)
        XCTAssertEqual(plan.kept.count, 2)
    }
}
