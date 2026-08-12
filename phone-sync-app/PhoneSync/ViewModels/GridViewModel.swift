import Foundation
import Photos

/// Drives the media grid: photo authorization, loading assets, and wiring the
/// library-change observer to auto-sync new captures.
@MainActor
final class GridViewModel: ObservableObject {
    @Published private(set) var assets: [PHAsset] = []
    @Published private(set) var authorizationDenied = false
    @Published private(set) var isLoading = false
    /// Items the server holds, shown in the Synced view (independent of local
    /// assets, since local synced items may be deleted later).
    @Published private(set) var serverItems: [MediaListItem] = []
    @Published private(set) var isLoadingServer = false

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

    /// Loads the server's media listing for the Synced view.
    func loadServerItems(using api: ApiClient, token: String) async {
        isLoadingServer = true
        defer { isLoadingServer = false }
        if let items = try? await api.fetchMediaList(token: token) {
            serverItems = items
        }
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
