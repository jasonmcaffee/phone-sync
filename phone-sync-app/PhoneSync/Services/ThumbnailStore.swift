import UIKit

/// Fetches and caches grid thumbnails for server (Synced-view) items so we don't
/// reload previews from the server every time. Resolution order:
///   in-memory cache → on-disk cache → server thumbnail.
/// The server generates thumbnails for every format (image crate for JPEG/PNG,
/// ffmpeg for HEIC/video), so the client just fetches and caches them.
final class ThumbnailStore {
    private let memory = NSCache<NSString, UIImage>()
    private let directory: URL
    private let api: ApiClient

    init(api: ApiClient) {
        self.api = api
        let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        directory = caches.appendingPathComponent("thumbnails", isDirectory: true)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        memory.countLimit = 500
    }

    /// Returns a thumbnail for a server item, caching it in memory and on disk.
    /// Returns nil if the server has no thumbnail yet (e.g. still generating).
    func thumbnail(for item: MediaListItem, token: String) async -> UIImage? {
        let key = item.id
        if let cached = memory.object(forKey: key as NSString) { return cached }
        if let disk = diskImage(key) {
            memory.setObject(disk, forKey: key as NSString)
            return disk
        }
        guard let image = await fetchServerThumbnail(id: key, token: token) else { return nil }
        memory.setObject(image, forKey: key as NSString)
        if let data = image.jpegData(compressionQuality: 0.85) {
            try? data.write(to: fileURL(key), options: .atomic)
        }
        return image
    }

    private func fileURL(_ key: String) -> URL { directory.appendingPathComponent("\(key).jpg") }

    private func diskImage(_ key: String) -> UIImage? {
        guard let data = try? Data(contentsOf: fileURL(key)) else { return nil }
        return UIImage(data: data)
    }

    /// Downloads the server's thumbnail for an item.
    private func fetchServerThumbnail(id: String, token: String) async -> UIImage? {
        guard let url = api.thumbnailURL(id: id, token: token),
              let (data, response) = try? await URLSession.shared.data(from: url),
              (response as? HTTPURLResponse)?.statusCode == 200 else { return nil }
        return UIImage(data: data)
    }
}
