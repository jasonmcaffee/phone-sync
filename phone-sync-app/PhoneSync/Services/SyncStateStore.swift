import Foundation

/// Persists the per-asset sync records (asset localIdentifier -> record) to a
/// JSON file in Application Support. This is the local source of truth for the
/// sync badges, reconciled against the server manifest on launch/sign-in.
final class SyncStateStore {
    private let fileURL: URL

    /// Opens the store at a fixed path under Application Support, creating the
    /// directory if needed. An override URL is accepted for tests.
    init(fileURL: URL? = nil) {
        if let fileURL = fileURL {
            self.fileURL = fileURL
        } else {
            let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
                .appendingPathComponent("PhoneSync", isDirectory: true)
            try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            self.fileURL = dir.appendingPathComponent("sync-state.json")
        }
        // UI-test hook: start from a clean slate so a sync run is observable.
        if ProcessInfo.processInfo.arguments.contains("-uitest-reset") {
            try? FileManager.default.removeItem(at: self.fileURL)
        }
    }

    /// Loads the persisted records, or an empty map if none/unreadable.
    func load() -> [String: StoredSyncRecord] {
        guard let data = try? Data(contentsOf: fileURL),
              let records = try? JSONDecoder().decode([String: StoredSyncRecord].self, from: data) else {
            return [:]
        }
        return records
    }

    /// Persists the full record map, atomically.
    func save(_ records: [String: StoredSyncRecord]) {
        guard let data = try? JSONEncoder().encode(records) else { return }
        try? data.write(to: fileURL, options: .atomic)
    }
}
