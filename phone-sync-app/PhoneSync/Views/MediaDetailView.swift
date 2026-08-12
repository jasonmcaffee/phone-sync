import SwiftUI
import Photos
import AVKit

/// Full-screen viewer: a horizontally paged gallery you can swipe through, with
/// pinch-to-zoom on photos and playback for videos. Works for both local assets
/// and server items.
struct MediaDetailView: View {
    let items: [DisplayItem]
    @State private var index: Int
    @Environment(\.dismiss) private var dismiss

    init(items: [DisplayItem], startIndex: Int) {
        self.items = items
        _index = State(initialValue: startIndex)
    }

    var body: some View {
        ZStack(alignment: .topLeading) {
            Color.black.ignoresSafeArea()

            TabView(selection: $index) {
                ForEach(items.indices, id: \.self) { i in
                    MediaPageView(item: items[i], pageIndex: i, currentIndex: index)
                        .tag(i)
                }
            }
            .tabViewStyle(.page(indexDisplayMode: .never))
            .ignoresSafeArea()

            controls
        }
    }

    /// Close button and caption overlay.
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
                Text("\(index + 1) of \(items.count)")
                    .font(.caption).foregroundStyle(.white.opacity(0.85))
                    .padding(.horizontal, 10).padding(.vertical, 5)
                    .background(.ultraThinMaterial, in: Capsule())
            }
            .padding()
            Spacer()
        }
    }
}

/// One page of the viewer: loads and shows a photo (zoomable) or video, but only
/// while it is the current or an adjacent page, so a long gallery doesn't hold
/// every full-resolution item in memory at once.
private struct MediaPageView: View {
    let item: DisplayItem
    let pageIndex: Int
    let currentIndex: Int

    @EnvironmentObject private var environment: AppEnvironment
    @EnvironmentObject private var auth: AuthService
    @State private var image: UIImage?
    @State private var player: AVPlayer?

    private var isNear: Bool { abs(pageIndex - currentIndex) <= 1 }
    private var isActive: Bool { pageIndex == currentIndex }

    var body: some View {
        ZStack {
            if item.isVideo {
                if let player {
                    VideoPlayer(player: player).accessibilityIdentifier("videoPlayer")
                } else {
                    ProgressView().tint(.white)
                }
            } else if let image {
                ZoomableImageView(image: image).accessibilityIdentifier("fullImage")
            } else {
                ProgressView().tint(.white)
            }
        }
        .task(id: currentIndex) { await refresh() }
        .onChange(of: isActive) { _, active in
            if active { player?.play() } else { player?.pause() }
        }
        .onDisappear { player?.pause() }
    }

    /// Loads media when this page is near the current one; releases it otherwise.
    private func refresh() async {
        guard isNear else {
            image = nil
            player?.pause()
            player = nil
            return
        }
        if item.isVideo {
            if player == nil { player = await makePlayer() }
            if isActive { player?.play() }
        } else if image == nil {
            image = await loadImage()
        }
    }

    /// Loads the full-resolution image (local via PhotoKit, remote via the server).
    private func loadImage() async -> UIImage? {
        switch item {
        case .local(let asset):
            return await environment.photoService.fullImage(for: asset)
        case .remote(let media):
            guard let token = auth.bearer,
                  let url = environment.api.mediaURL(id: media.id, token: token),
                  let (data, _) = try? await URLSession.shared.data(from: url) else { return nil }
            return UIImage(data: data)
        }
    }

    /// Builds an AVPlayer for the video (local via PhotoKit, remote streamed).
    private func makePlayer() async -> AVPlayer? {
        switch item {
        case .local(let asset):
            guard let playerItem = await environment.photoService.playerItem(for: asset) else { return nil }
            return AVPlayer(playerItem: playerItem)
        case .remote(let media):
            guard let token = auth.bearer,
                  let url = environment.api.mediaURL(id: media.id, token: token) else { return nil }
            return AVPlayer(url: url)
        }
    }
}

/// A pinch-to-zoom, pan-when-zoomed image. At 1× the pan gesture is disabled so
/// a horizontal swipe pages the parent gallery instead; double-tap toggles zoom.
private struct ZoomableImageView: View {
    let image: UIImage

    @State private var scale: CGFloat = 1
    @GestureState private var pinch: CGFloat = 1
    @State private var offset: CGSize = .zero
    @State private var lastOffset: CGSize = .zero

    var body: some View {
        let magnify = MagnificationGesture()
            .updating($pinch) { value, state, _ in state = value }
            .onEnded { value in
                scale = min(max(scale * value, 1), 6)
                if scale == 1 { offset = .zero; lastOffset = .zero }
            }

        let pan = DragGesture()
            .onChanged { value in
                offset = CGSize(width: lastOffset.width + value.translation.width,
                                height: lastOffset.height + value.translation.height)
            }
            .onEnded { _ in lastOffset = offset }

        Image(uiImage: image)
            .resizable()
            .scaledToFit()
            .scaleEffect(scale * pinch)
            .offset(offset)
            .gesture(magnify)
            // Only intercept drags while zoomed in; at 1× let the TabView page.
            .highPriorityGesture(pan, including: scale > 1 ? .all : .subviews)
            .onTapGesture(count: 2) {
                withAnimation(.spring(response: 0.3)) {
                    if scale > 1 {
                        scale = 1; offset = .zero; lastOffset = .zero
                    } else {
                        scale = 2.5
                    }
                }
            }
    }
}
