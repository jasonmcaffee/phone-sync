import SwiftUI
import Photos

/// A single grid cell: the asset's thumbnail with a video-duration overlay and
/// a sync-state badge in the corner.
struct MediaCellView: View {
    let asset: PHAsset
    let state: SyncState

    @EnvironmentObject private var environment: AppEnvironment
    @State private var thumbnail: UIImage?

    var body: some View {
        GeometryReader { geo in
            ZStack {
                thumbnailLayer(size: geo.size)
                overlays
            }
            .frame(width: geo.size.width, height: geo.size.height)
            .clipped()
        }
        .task(id: asset.localIdentifier) {
            let side = 440.0   // ~2x thumbnail resolution for retina displays
            thumbnail = await environment.photoService.thumbnail(
                for: asset,
                targetSize: CGSize(width: side, height: side)
            )
        }
    }

    /// The image (or a placeholder while loading).
    @ViewBuilder
    private func thumbnailLayer(size: CGSize) -> some View {
        if let thumbnail = thumbnail {
            Image(uiImage: thumbnail)
                .resizable()
                .scaledToFill()
        } else {
            Rectangle()
                .fill(Color(.secondarySystemBackground))
                .overlay(ProgressView())
        }
    }

    /// Video badge (top-left) and sync badge (bottom-right).
    private var overlays: some View {
        VStack {
            HStack {
                if asset.mediaType == .video {
                    Image(systemName: "video.fill")
                        .font(.caption2)
                        .foregroundStyle(.white)
                        .shadow(radius: 2)
                    Text(durationText)
                        .font(.caption2)
                        .foregroundStyle(.white)
                        .shadow(radius: 2)
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

    /// The colored sync-state indicator.
    private var syncBadge: some View {
        Image(systemName: badgeSymbol)
            .font(.system(size: 16))
            .symbolRenderingMode(.palette)
            .foregroundStyle(.white, badgeColor)
            .background(Circle().fill(.black.opacity(0.25)).frame(width: 22, height: 22))
            .accessibilityIdentifier("badge-\(state.rawValue)")
    }

    /// SF Symbol name for the current state.
    private var badgeSymbol: String {
        switch state {
        case .notSynced: return "icloud.slash.fill"
        case .syncing: return "arrow.triangle.2.circlepath.circle.fill"
        case .synced: return "checkmark.circle.fill"
        case .failed: return "exclamationmark.circle.fill"
        }
    }

    /// Accent color for the current state.
    private var badgeColor: Color {
        switch state {
        case .notSynced: return .gray
        case .syncing: return .blue
        case .synced: return .green
        case .failed: return .red
        }
    }

    /// Formats the video duration as m:ss.
    private var durationText: String {
        let total = Int(asset.duration.rounded())
        return String(format: "%d:%02d", total / 60, total % 60)
    }
}
