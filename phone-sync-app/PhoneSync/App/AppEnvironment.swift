import Foundation
import Photos

/// Composition root: constructs and holds the shared service instances, wiring
/// their dependencies together. Injected into the SwiftUI view hierarchy.
@MainActor
final class AppEnvironment: ObservableObject {
    let api: ApiClient
    let auth: AuthService
    let photoService: PhotoLibraryService
    let syncEngine: SyncEngine
    let gridViewModel: GridViewModel

    /// Builds the object graph. The API client's base URL comes from AuthService
    /// (which loads the persisted server URL).
    init() {
        let api = ApiClient(baseURLString: ServerConfig.defaultServer.baseURL)
        let auth = AuthService(api: api)
        let photoService = PhotoLibraryService()
        let store = SyncStateStore()
        let syncEngine = SyncEngine(photoService: photoService, api: api, auth: auth, store: store)

        self.api = api
        self.auth = auth
        self.photoService = photoService
        self.syncEngine = syncEngine
        self.gridViewModel = GridViewModel(photoService: photoService)
    }

    /// Runs a full sync pass over the current photo library. Shared by the
    /// manual button, the library-change observer, and the background task.
    /// Calls `completion` with success once done.
    func runSyncPass(completion: (@Sendable (Bool) -> Void)? = nil) {
        Task { @MainActor in
            let assets = photoService.fetchAssets()
            await syncEngine.syncAll(assets: assets)
            completion?(syncEngine.lastError == nil)
        }
    }
}
