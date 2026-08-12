import SwiftUI
import Photos

/// A single grid cell for a local or remote item: its thumbnail with a video
/// badge and, for local items, a sync-state badge.
struct MediaCellView: View {
    let item: DisplayItem
    let syncState: SyncState

    @EnvironmentObject private var environment: AppEnvironment
    @EnvironmentObject private var auth: AuthService
    @State private var localThumb: UIImage?

    var body: some View {
        GeometryReader { geo in
            ZStack {
                thumbnailLayer
                overlays
            }
            .frame(width: geo.size.width, height: geo.size.height)
            .clipped()
        }
        .task(id: item.id) { await loadLocalThumbIfNeeded() }
    }

    // MARK: - Thumbnail

    @ViewBuilder
    private var thumbnailLayer: some View {
        switch item {
        case .local:
            if let localThumb {
                Image(uiImage: localThumb).resizable().scaledToFill()
            } else {
                placeholder
            }
        case .remote(let media):
            remoteThumbnail(media)
        }
    }

    /// Loads the PhotoKit thumbnail for local items.
    private func loadLocalThumbIfNeeded() async {
        guard case .local(let asset) = item else { return }
        localThumb = await environment.photoService.thumbnail(for: asset, targetSize: CGSize(width: 440, height: 440))
    }

    /// Remote thumbnail: the server's cached thumbnail for decodable images; a
    /// labeled tile for videos and formats the server can't thumbnail (e.g.
    /// HEIC). We deliberately don't download full HEICs just for grid cells —
    /// they're shown full-size only in the viewer.
    @ViewBuilder
    private func remoteThumbnail(_ media: MediaListItem) -> some View {
        if media.thumbnailable, let token = auth.bearer,
           let url = environment.api.thumbnailURL(id: media.id, token: token) {
            AsyncImage(url: url) { phase in
                switch phase {
                case .success(let image): image.resizable().scaledToFill()
                case .empty: placeholder.overlay(ProgressView())
                default: fallbackTile(media)
                }
            }
        } else {
            fallbackTile(media)
        }
    }

    /// Tile for items with no server thumbnail (HEIC, video, or a load failure):
    /// the file type + name, with a film icon for videos.
    private func fallbackTile(_ media: MediaListItem) -> some View {
        placeholder.overlay(
            VStack(spacing: 4) {
                Image(systemName: media.mediaType == "video" ? "film" : "photo")
                    .font(.title3).foregroundStyle(.secondary)
                Text(fileExtension(media.filename)).font(.caption).fontWeight(.semibold)
                Text(media.filename).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
            }.padding(4)
        )
    }

    private var placeholder: some View {
        Rectangle().fill(Color(.secondarySystemBackground))
    }

    // MARK: - Overlays

    private var overlays: some View {
        VStack {
            HStack {
                if item.isVideo {
                    Image(systemName: "video.fill").font(.caption2).foregroundStyle(.white).shadow(radius: 2)
                    if item.duration > 0 {
                        Text(durationText).font(.caption2).foregroundStyle(.white).shadow(radius: 2)
                    }
                }
                Spacer()
            }
            Spacer()
            HStack {
                Spacer()
                syncBadge
            }
        }
        .padding(4)
    }

    /// Sync badge — only for local items (remote items are, by definition, on
    /// the server). Synced gets a green check; others their own glyph.
    @ViewBuilder
    private var syncBadge: some View {
        if case .local = item, let symbol = badgeSymbol {
            Image(systemName: symbol)
                .font(.system(size: 18, weight: .bold))
                .symbolRenderingMode(.palette)
                .foregroundStyle(.white, badgeColor)
                .background(Circle().fill(.white).frame(width: 15, height: 15))
                .shadow(color: .black.opacity(0.35), radius: 1.5, y: 0.5)
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
