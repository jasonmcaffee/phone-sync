import Foundation
import Photos

/// A grid/viewer item that may come from the local photo library (All/Unsynced
/// views) or from the server (Synced view). This lets one grid and one viewer
/// present both sources.
enum DisplayItem: Identifiable {
    case local(PHAsset)
    case remote(MediaListItem)

    var id: String {
        switch self {
        case .local(let asset): return "local-\(asset.localIdentifier)"
        case .remote(let item): return "remote-\(item.id)"
        }
    }

    /// True for videos (drives the play badge and viewer behavior).
    var isVideo: Bool {
        switch self {
        case .local(let asset): return asset.mediaType == .video
        case .remote(let item): return item.mediaType == "video"
        }
    }

    /// Original filename, used for fallback tiles and captions.
    var filename: String {
        switch self {
        case .local(let asset): return (asset.value(forKey: "filename") as? String) ?? "Photo"
        case .remote(let item): return item.filename
        }
    }

    /// Capture date used for grouping and ordering.
    var date: Date? {
        switch self {
        case .local(let asset): return asset.creationDate
        case .remote(let item): return DisplayItem.parseISO(item.createdAt)
        }
    }

    /// Video duration in seconds (0 for photos).
    var duration: Double {
        switch self {
        case .local(let asset): return asset.duration
        case .remote: return 0
        }
    }

    /// Parses an ISO-8601 timestamp (with or without fractional seconds).
    static func parseISO(_ string: String) -> Date? {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: string) { return date }
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: string)
    }
}

/// How the grid groups items by capture date.
enum GroupMode: String, CaseIterable, Identifiable {
    case none = "None"
    case day = "Day"
    case month = "Month"
    case year = "Year"
    var id: String { rawValue }
    var menuLabel: String { self == .none ? "No Grouping" : "By \(rawValue)" }
}

/// A titled group of items for a sectioned grid.
struct MediaSection: Identifiable {
    let id: String
    let title: String
    let items: [DisplayItem]
}

/// Groups an already date-ordered (newest-first) item list into titled sections.
enum MediaGrouping {
    private static let dayFormatter: DateFormatter = formatter("MMMM d, yyyy")
    private static let monthFormatter: DateFormatter = formatter("MMMM yyyy")
    private static let yearFormatter: DateFormatter = formatter("yyyy")

    private static func formatter(_ format: String) -> DateFormatter {
        let f = DateFormatter()
        f.dateFormat = format
        return f
    }

    /// Splits `items` into contiguous sections by the chosen grouping. Input is
    /// assumed newest-first, so sections come out newest-first too.
    static func sections(_ items: [DisplayItem], mode: GroupMode) -> [MediaSection] {
        guard mode != .none else {
            return items.isEmpty ? [] : [MediaSection(id: "all", title: "", items: items)]
        }
        var sections: [MediaSection] = []
        var currentKey: String?
        var bucket: [DisplayItem] = []
        for item in items {
            let key = title(for: item.date, mode: mode)
            if key != currentKey {
                if let currentKey { sections.append(MediaSection(id: currentKey, title: currentKey, items: bucket)) }
                currentKey = key
                bucket = [item]
            } else {
                bucket.append(item)
            }
        }
        if let currentKey { sections.append(MediaSection(id: currentKey, title: currentKey, items: bucket)) }
        return sections
    }

    /// Section title for a date under the given grouping.
    private static func title(for date: Date?, mode: GroupMode) -> String {
        guard let date else { return "Unknown Date" }
        switch mode {
        case .day: return dayFormatter.string(from: date)
        case .month: return monthFormatter.string(from: date)
        case .year: return yearFormatter.string(from: date)
        case .none: return ""
        }
    }
}
