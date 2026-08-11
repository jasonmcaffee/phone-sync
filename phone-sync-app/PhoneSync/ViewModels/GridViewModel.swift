import Foundation
import Photos

/// Drives the media grid: photo authorization, loading assets, and wiring the
/// library-change observer to auto-sync new captures.
@MainActor
final class GridViewModel: ObservableObject {
    @Published private(set) var assets: [PHAsset] = []
    @Published private(set) var authorizationDenied = false
    @Published private(set) var isLoading = false

    private let photoService: PhotoLibraryService

    /// Injects the photo library service.
    init(photoService: PhotoLibraryService) {
        self.photoService = photoService
    }

    /// Requests authorization if needed, then loads all photos and videos.
    func load() async {
        isLoading = true
        defer { isLoading = false }

        var status = photoService.authorizationStatus
        if status == .notDetermined {
            status = await photoService.requestAuthorization()
        }
        guard status == .authorized || status == .limited else {
            authorizationDenied = true
            return
        }
        authorizationDenied = false
        assets = photoService.fetchAssets()
    }

    /// Reloads the asset list (e.g. after the library changes).
    func reload() {
        assets = photoService.fetchAssets()
    }

    /// Starts observing the photo library; on change, reloads the grid and
    /// invokes `onChange` so the caller can kick off an automatic sync.
    func startAutoSync(onChange: @escaping () -> Void) {
        photoService.startObserving { [weak self] in
            self?.reload()
            onChange()
        }
    }
}
