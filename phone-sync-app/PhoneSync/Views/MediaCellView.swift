import SwiftUI
import Photos

/// A single grid cell for a local or remote item: its thumbnail with a video
/// play overlay and, for local items, a sync-state badge. Laid out as a fixed
/// square with badge overlays, so a `scaledToFill` thumbnail can't push the
/// badges outside the visible area.
struct MediaCellView: View {
    let item: DisplayItem
    let syncState: SyncState

    @EnvironmentObject private var environment: AppEnvironment
    @EnvironmentObject private var auth: AuthService
    @State private var thumb: UIImage?
    @State private var loadFailed = false

    var body: some View {
        Color(.secondarySystemBackground)
            .overlay { thumbnailContent }
            .clipped()
            .overlay(alignment: .topLeading) { durationTag }
            .overlay { playIcon }
            .overlay(alignment: .bottomTrailing) { syncBadge }
            .contentShape(Rectangle())
            .task(id: item.id) { await load() }
    }

    // MARK: - Thumbnail

    @ViewBuilder
    private var thumbnailContent: some View {
        if let thumb {
            Image(uiImage: thumb).resizable().scaledToFill()
        } else if loadFailed {
            fallbackTile
        } else {
            ProgressView()
        }
    }

    /// Loads the thumbnail: PhotoKit for local items, the ThumbnailStore (server
    /// + cache) for remote ones.
    private func load() async {
        loadFailed = false
        switch item {
        case .local(let asset):
            thumb = await environment.photoService.thumbnail(for: asset, targetSize: CGSize(width: 440, height: 440))
        case .remote(let media):
            guard let token = auth.bearer else { loadFailed = true; return }
            thumb = await environment.thumbnailStore.thumbnail(for: media, token: token)
        }
        if thumb == nil { loadFailed = true }
    }

    /// Tile shown when no thumbnail is available (server still generating, etc.).
    private var fallbackTile: some View {
        VStack(spacing: 4) {
            Image(systemName: item.isVideo ? "film" : "photo")
                .font(.title3).foregroundStyle(.secondary)
            Text(fileExtension(item.filename)).font(.caption).fontWeight(.semibold)
        }
        .padding(4)
    }

    // MARK: - Overlays

    /// Video duration in the top-left corner.
    @ViewBuilder
    private var durationTag: some View {
        if item.isVideo && item.duration > 0 {
            Text(durationText)
                .font(.caption2).foregroundStyle(.white).shadow(radius: 2)
                .padding(5)
        }
    }

    /// Centered play glyph on video thumbnails.
    @ViewBuilder
    private var playIcon: some View {
        if item.isVideo && thumb != nil {
            Image(systemName: "play.circle.fill")
                .font(.title)
                .symbolRenderingMode(.palette)
                .foregroundStyle(.white, .black.opacity(0.35))
                .shadow(radius: 3)
        }
    }

    /// Sync badge — only for local items (remote items are, by definition, on
    /// the server). Synced gets a green check.
    @ViewBuilder
    private var syncBadge: some View {
        if case .local = item, let symbol = badgeSymbol {
            Image(systemName: symbol)
                .font(.system(size: 18, weight: .bold))
                .symbolRenderingMode(.palette)
                .foregroundStyle(.white, badgeColor)
                .background(Circle().fill(.white).frame(width: 15, height: 15))
                .shadow(color: .black.opacity(0.35), radius: 1.5, y: 0.5)
                .padding(4)
                .accessibilityIdentifier("badge-\(syncState.rawValue)")
        }
    }

    private var badgeSymbol: String? {
        switch syncState {
        case .notSynced: return nil
        case .syncing: return "arrow.triangle.2.circlepath.circle.fill"
        case .synced: return "checkmark.circle.fill"
        case .failed: return "exclamationmark.circle.fill"
        }
    }

    private var badgeColor: Color {
        switch syncState {
        case .notSynced: return .gray
        case .syncing: return .blue
        case .synced: return .green
        case .failed: return .red
        }
    }

    private var durationText: String {
        let total = Int(item.duration.rounded())
        return String(format: "%d:%02d", total / 60, total % 60)
    }

    private func fileExtension(_ filename: String) -> String {
        let dot = filename.lastIndex(of: ".")
        return dot.map { String(filename[filename.index(after: $0)...]).uppercased() } ?? "FILE"
    }
}
