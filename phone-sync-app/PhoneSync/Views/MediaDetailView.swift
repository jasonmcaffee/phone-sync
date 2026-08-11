import SwiftUI
import Photos
import AVKit

/// Full-screen viewer for a single asset. Shows a zoomable photo or a playable
/// video, a sync-state chip, and a close button.
struct MediaDetailView: View {
    let asset: PHAsset

    @EnvironmentObject private var environment: AppEnvironment
    @EnvironmentObject private var syncEngine: SyncEngine
    @Environment(\.dismiss) private var dismiss

    @State private var image: UIImage?
    @State private var player: AVPlayer?

    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()
            content
            controls
        }
        .task { await loadMedia() }
        .onDisappear { player?.pause() }
    }

    /// Photo or video content depending on the asset's media type.
    @ViewBuilder
    private var content: some View {
        if asset.mediaType == .video {
            if let player = player {
                VideoPlayer(player: player)
                    .ignoresSafeArea()
                    .accessibilityIdentifier("videoPlayer")
            } else {
                ProgressView().tint(.white)
            }
        } else {
            if let image = image {
                Image(uiImage: image)
                    .resizable()
                    .scaledToFit()
                    .accessibilityIdentifier("fullImage")
            } else {
                ProgressView().tint(.white)
            }
        }
    }

    /// Close button and sync-state chip overlay.
    private var controls: some View {
        VStack {
            HStack {
                Button { dismiss() } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.title)
                        .symbolRenderingMode(.palette)
                        .foregroundStyle(.white, .black.opacity(0.4))
                }
                .accessibilityIdentifier("closeDetail")
                Spacer()
                syncChip
            }
            .padding()
            Spacer()
        }
    }

    /// A small labeled chip reflecting the asset's sync state.
    private var syncChip: some View {
        let state = syncEngine.state(for: asset.localIdentifier)
        return Label(label(for: state), systemImage: symbol(for: state))
            .font(.caption)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(.ultraThinMaterial)
            .clipShape(Capsule())
            .foregroundStyle(.white)
    }

    /// Loads the appropriate media for display.
    private func loadMedia() async {
        if asset.mediaType == .video {
            player = await environment.photoService.playerItem(for: asset).map { AVPlayer(playerItem: $0) }
            player?.play()
        } else {
            image = await environment.photoService.fullImage(for: asset)
        }
    }

    /// Human-readable label for a sync state.
    private func label(for state: SyncState) -> String {
        switch state {
        case .notSynced: return "Not synced"
        case .syncing: return "Syncing…"
        case .synced: return "Synced"
        case .failed: return "Failed"
        }
    }

    /// SF Symbol for a sync state.
    private func symbol(for state: SyncState) -> String {
        switch state {
        case .notSynced: return "icloud.slash"
        case .syncing: return "arrow.triangle.2.circlepath"
        case .synced: return "checkmark.icloud"
        case .failed: return "exclamationmark.icloud"
        }
    }
}
