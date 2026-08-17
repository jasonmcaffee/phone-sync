import Foundation
import Photos
import UIKit
import AVFoundation
import UniformTypeIdentifiers

/// Error thrown when a photo-library deletion is cancelled by the user or fails.
enum PhotoDeletionError: Error { case cancelled }

/// Lets PHAsset be used directly with SwiftUI's item-based presentations.
extension PHAsset: Identifiable {
    public var id: String { localIdentifier }
}

/// Full-resolution export of a single asset ready for upload.
struct ExportedAsset {
    let data: Data
    let filename: String
    let contentType: String
    let mediaType: String   // "photo" | "video"
}

/// Full-resolution export written to a temporary file on disk, so large videos
/// never have to be held whole in memory. The caller deletes `url` when done.
struct ExportedFile {
    let url: URL
    let filename: String
    let contentType: String
    let mediaType: String
    let size: Int
}

/// Wraps PhotoKit: authorization, fetching photos/videos, thumbnail rendering,
/// full-resolution export for upload, and change observation so new captures
/// can trigger a sync.
final class PhotoLibraryService: NSObject, PHPhotoLibraryChangeObserver {
    private let imageManager = PHCachingImageManager()
    private var changeHandler: (() -> Void)?

    /// Requests read authorization, returning the resulting status.
    func requestAuthorization() async -> PHAuthorizationStatus {
        await withCheckedContinuation { continuation in
            PHPhotoLibrary.requestAuthorization(for: .readWrite) { status in
                continuation.resume(returning: status)
            }
        }
    }

    /// Current authorization status without prompting.
    var authorizationStatus: PHAuthorizationStatus {
        PHPhotoLibrary.authorizationStatus(for: .readWrite)
    }

    /// Fetches all image and video assets, newest first.
    func fetchAssets() -> [PHAsset] {
        let options = PHFetchOptions()
        options.sortDescriptors = [NSSortDescriptor(key: "creationDate", ascending: false)]
        options.predicate = NSPredicate(
            format: "mediaType == %d OR mediaType == %d",
            PHAssetMediaType.image.rawValue, PHAssetMediaType.video.rawValue
        )
        let result = PHAsset.fetchAssets(with: options)
        var assets: [PHAsset] = []
        assets.reserveCapacity(result.count)
        result.enumerateObjects { asset, _, _ in assets.append(asset) }
        return assets
    }

    /// Looks up a local asset by its localIdentifier (used to generate a preview
    /// for a server item that is still on this device).
    func asset(withLocalIdentifier id: String) -> PHAsset? {
        PHAsset.fetchAssets(withLocalIdentifiers: [id], options: nil).firstObject
    }

    /// Returns the subset of the given localIdentifiers that still exist on this
    /// device (used to find server items whose local copy can be deleted).
    func existingLocalIdentifiers(_ ids: [String]) -> Set<String> {
        let result = PHAsset.fetchAssets(withLocalIdentifiers: ids, options: nil)
        var present = Set<String>()
        result.enumerateObjects { asset, _, _ in present.insert(asset.localIdentifier) }
        return present
    }

    /// Exact byte size of an asset's primary resource, or nil if unavailable.
    /// Used as a deletion pre-check (the server confirms its stored size matches).
    func resourceByteSize(for asset: PHAsset) -> Int64? {
        let resources = PHAssetResource.assetResources(for: asset)
        guard let resource = primaryResource(from: resources, mediaType: asset.mediaType) else { return nil }
        return (resource.value(forKey: "fileSize") as? NSNumber)?.int64Value
    }

    /// Deletes assets from the photo library. iOS shows its own delete
    /// confirmation; throws if the user cancels or the change fails. This removes
    /// the items from Photos entirely (and from iCloud Photos if enabled).
    func deleteAssets(withLocalIdentifiers ids: [String]) async throws {
        let assets = PHAsset.fetchAssets(withLocalIdentifiers: ids, options: nil)
        guard assets.count > 0 else { return }
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            PHPhotoLibrary.shared().performChanges {
                PHAssetChangeRequest.deleteAssets(assets)
            } completionHandler: { success, error in
                if success {
                    continuation.resume()
                } else {
                    continuation.resume(throwing: error ?? PhotoDeletionError.cancelled)
                }
            }
        }
    }

    /// Renders a thumbnail for grid display at approximately `targetSize`.
    func thumbnail(for asset: PHAsset, targetSize: CGSize) async -> UIImage? {
        await withCheckedContinuation { continuation in
            let options = PHImageRequestOptions()
            options.deliveryMode = .opportunistic
            options.resizeMode = .fast
            options.isNetworkAccessAllowed = true
            var resumed = false
            imageManager.requestImage(for: asset, targetSize: targetSize, contentMode: .aspectFill, options: options) { image, info in
                // Opportunistic delivery may call back twice; resume once on the
                // final (non-degraded) image, or on the first if it's final.
                let isDegraded = (info?[PHImageResultIsDegradedKey] as? Bool) ?? false
                if !resumed && (!isDegraded || image != nil) {
                    if isDegraded { return }
                    resumed = true
                    continuation.resume(returning: image)
                }
            }
        }
    }

    /// Exports the full-resolution original bytes + metadata for upload.
    /// Chooses the primary photo or full-size video resource.
    func exportForUpload(_ asset: PHAsset) async -> ExportedAsset? {
        let resources = PHAssetResource.assetResources(for: asset)
        guard let resource = primaryResource(from: resources, mediaType: asset.mediaType) else {
            return nil
        }
        guard let data = await accumulateData(from: resource) else { return nil }

        let contentType = UTType(resource.uniformTypeIdentifier)?.preferredMIMEType
            ?? (asset.mediaType == .video ? "video/quicktime" : "image/jpeg")
        let mediaType = asset.mediaType == .video ? "video" : "photo"
        return ExportedAsset(data: data, filename: resource.originalFilename, contentType: contentType, mediaType: mediaType)
    }

    /// Exports the full-resolution original to a temporary file, streaming bytes
    /// straight to disk via `writeData(for:toFile:)` so a multi-GB video is never
    /// buffered in memory (which would jetsam-kill the app). Returns nil if the
    /// resource can't be written. The caller must delete the returned file.
    func exportToTempFile(_ asset: PHAsset) async -> ExportedFile? {
        let resources = PHAssetResource.assetResources(for: asset)
        guard let resource = primaryResource(from: resources, mediaType: asset.mediaType) else {
            return nil
        }
        let contentType = UTType(resource.uniformTypeIdentifier)?.preferredMIMEType
            ?? (asset.mediaType == .video ? "video/quicktime" : "image/jpeg")
        let mediaType = asset.mediaType == .video ? "video" : "photo"
        let tmpURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("phonesync-\(UUID().uuidString)-\(resource.originalFilename)")

        let options = PHAssetResourceRequestOptions()
        options.isNetworkAccessAllowed = true
        let wrote: Bool = await withCheckedContinuation { continuation in
            PHAssetResourceManager.default().writeData(for: resource, toFile: tmpURL, options: options) { error in
                continuation.resume(returning: error == nil)
            }
        }
        guard wrote else {
            try? FileManager.default.removeItem(at: tmpURL)
            return nil
        }
        let attrs = try? FileManager.default.attributesOfItem(atPath: tmpURL.path)
        let size = (attrs?[.size] as? NSNumber)?.intValue ?? 0
        return ExportedFile(url: tmpURL, filename: resource.originalFilename, contentType: contentType, mediaType: mediaType, size: size)
    }

    /// Best-effort byte size of an asset's primary resource, used to sync the
    /// smallest items first. Reads the resource's `fileSize` (metadata only, no
    /// data transfer); falls back to a heuristic so photos still sort ahead of
    /// videos and shorter videos ahead of longer ones when size is unavailable.
    func estimatedByteSize(for asset: PHAsset) -> Int64 {
        let resources = PHAssetResource.assetResources(for: asset)
        if let resource = primaryResource(from: resources, mediaType: asset.mediaType),
           let size = (resource.value(forKey: "fileSize") as? NSNumber)?.int64Value {
            return size
        }
        if asset.mediaType == .video {
            return Int64(max(1, asset.duration)) * 5_000_000 // ~5 MB per second
        }
        return Int64(asset.pixelWidth) * Int64(asset.pixelHeight) // rough photo proxy
    }

    /// Picks the best resource to upload for the given media type.
    private func primaryResource(from resources: [PHAssetResource], mediaType: PHAssetMediaType) -> PHAssetResource? {
        if mediaType == .video {
            return resources.first(where: { $0.type == .video })
                ?? resources.first(where: { $0.type == .fullSizeVideo })
                ?? resources.first
        }
        return resources.first(where: { $0.type == .photo })
            ?? resources.first(where: { $0.type == .fullSizePhoto })
            ?? resources.first
    }

    /// Streams a resource's bytes into an in-memory Data buffer.
    private func accumulateData(from resource: PHAssetResource) async -> Data? {
        await withCheckedContinuation { continuation in
            let options = PHAssetResourceRequestOptions()
            options.isNetworkAccessAllowed = true
            var buffer = Data()
            PHAssetResourceManager.default().requestData(for: resource, options: options) { chunk in
                buffer.append(chunk)
            } completionHandler: { error in
                continuation.resume(returning: error == nil ? buffer : nil)
            }
        }
    }

    /// Loads a full-resolution image for the detail viewer.
    func fullImage(for asset: PHAsset) async -> UIImage? {
        await withCheckedContinuation { continuation in
            let options = PHImageRequestOptions()
            options.deliveryMode = .highQualityFormat
            options.isNetworkAccessAllowed = true
            options.resizeMode = .none
            var resumed = false
            imageManager.requestImage(for: asset, targetSize: PHImageManagerMaximumSize, contentMode: .aspectFit, options: options) { image, info in
                let isDegraded = (info?[PHImageResultIsDegradedKey] as? Bool) ?? false
                if isDegraded { return }
                if !resumed {
                    resumed = true
                    continuation.resume(returning: image)
                }
            }
        }
    }

    /// Loads an AVPlayerItem for video playback in the detail viewer.
    func playerItem(for asset: PHAsset) async -> AVPlayerItem? {
        await withCheckedContinuation { continuation in
            let options = PHVideoRequestOptions()
            options.deliveryMode = .automatic
            options.isNetworkAccessAllowed = true
            PHImageManager.default().requestPlayerItem(forVideo: asset, options: options) { item, _ in
                continuation.resume(returning: item)
            }
        }
    }

    // MARK: - Change observation

    /// Registers a handler invoked whenever the photo library changes (e.g. a
    /// new capture), so the sync engine can pick up new media automatically.
    func startObserving(_ handler: @escaping () -> Void) {
        changeHandler = handler
        PHPhotoLibrary.shared().register(self)
    }

    /// Stops observing library changes.
    func stopObserving() {
        PHPhotoLibrary.shared().unregisterChangeObserver(self)
        changeHandler = nil
    }

    /// PHPhotoLibraryChangeObserver callback (may be off the main thread).
    func photoLibraryDidChange(_ changeInstance: PHChange) {
        let handler = changeHandler
        DispatchQueue.main.async { handler?() }
    }
}
