import Foundation

/// Errors surfaced by the API layer.
enum ApiError: LocalizedError {
    case badURL
    case unauthorized
    case server(status: Int, message: String)
    case decoding(String)
    case transport(String)

    var errorDescription: String? {
        switch self {
        case .badURL: return "Invalid server URL."
        case .unauthorized: return "Your session expired. Please sign in again."
        case .server(let status, let message): return "Server error \(status): \(message)"
        case .decoding(let m): return "Could not read server response: \(m)"
        case .transport(let m): return "Network error: \(m)"
        }
    }
}

/// HTTP client for the Phone Sync backend. Stateless with respect to auth —
/// callers pass the bearer token. Uses async/await over URLSession.
final class ApiClient {
    private let session: URLSession
    private var baseURL: URL?

    /// Creates a client; `baseURLString` may be updated later via `configure`.
    init(baseURLString: String, session: URLSession = .shared) {
        self.session = session
        self.baseURL = URL(string: baseURLString)
    }

    /// Points the client at a new base URL (e.g. when the user edits Settings).
    func configure(baseURLString: String) {
        self.baseURL = URL(string: baseURLString)
    }

    /// Signs in and returns the token + expiry.
    func login(username: String, password: String) async throws -> LoginResponse {
        let request = try makeRequest(path: "/auth/login", method: "POST", token: nil, jsonBody: LoginRequest(username: username, password: password))
        return try await send(request, decode: LoginResponse.self)
    }

    /// Fetches the set of asset ids the server already stores.
    func fetchManifest(token: String) async throws -> Set<String> {
        let request = try makeRequest(path: "/media/manifest", method: "GET", token: token, jsonBody: Optional<LoginRequest>.none)
        let response = try await send(request, decode: ManifestResponse.self)
        return Set(response.assetIds)
    }

    /// Uploads one asset with its metadata as multipart/form-data, streaming the
    /// request body from a temp file so the body is never held whole in memory.
    func upload(metadata: UploadMetadata, fileData: Data, token: String) async throws -> UploadResponse {
        guard let base = baseURL, let url = URL(string: "/media/upload", relativeTo: base) else {
            throw ApiError.badURL
        }
        let boundary = "PhoneSyncBoundary-\(UUID().uuidString)"
        let bodyURL = try writeMultipartFileBody(metadata: metadata, fileData: fileData, boundary: boundary)
        defer { try? FileManager.default.removeItem(at: bodyURL) }
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        return try await sendUpload(request, fromFile: bodyURL, decode: UploadResponse.self)
    }

    // MARK: - Chunked upload (files larger than the edge's body limit)

    /// Queries the server for whether a file's content is already stored and
    /// which chunk indices it already holds, so the client can skip/resume.
    func uploadStatus(sha256: String, token: String) async throws -> ChunkStatusResponse {
        let request = try makeRequest(path: "/media/upload/status/\(sha256)", method: "GET", token: token, jsonBody: Optional<CompleteRequest>.none)
        return try await send(request, decode: ChunkStatusResponse.self)
    }

    /// Uploads a single chunk, streaming the multipart body from a temp file so
    /// large chunks don't accumulate in memory (which was OOM-killing the app).
    func uploadChunk(sha256: String, chunkIndex: Int, chunkData: Data, token: String) async throws {
        guard let base = baseURL, let url = URL(string: "/media/upload/chunk", relativeTo: base) else {
            throw ApiError.badURL
        }
        let boundary = "PhoneSyncChunk-\(UUID().uuidString)"
        let bodyURL = try writeChunkFileBody(sha256: sha256, chunkIndex: chunkIndex, chunkData: chunkData, boundary: boundary)
        defer { try? FileManager.default.removeItem(at: bodyURL) }
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        _ = try await sendUpload(request, fromFile: bodyURL, decode: ChunkAck.self)
    }

    /// Finalizes a chunked upload; the server assembles and verifies the chunks.
    func uploadComplete(_ completeRequest: CompleteRequest, token: String) async throws -> UploadResponse {
        let request = try makeRequest(path: "/media/upload/complete", method: "POST", token: token, jsonBody: completeRequest)
        return try await send(request, decode: UploadResponse.self)
    }

    /// Writes a full asset upload's multipart body (metadata + file) to a temp
    /// file and returns its URL. The caller deletes it after the upload.
    private func writeMultipartFileBody(metadata: UploadMetadata, fileData: Data, boundary: String) throws -> URL {
        let bodyURL = FileManager.default.temporaryDirectory.appendingPathComponent("psync-body-\(UUID().uuidString)")
        FileManager.default.createFile(atPath: bodyURL.path, contents: nil)
        let handle = try FileHandle(forWritingTo: bodyURL)
        defer { try? handle.close() }
        let metadataJSON = try JSONEncoder().encode(metadata)
        handle.write(Data("--\(boundary)\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n".utf8))
        handle.write(metadataJSON)
        handle.write(Data("\r\n--\(boundary)\r\nContent-Disposition: form-data; name=\"file\"; filename=\"\(metadata.filename)\"\r\nContent-Type: \(metadata.contentType)\r\n\r\n".utf8))
        handle.write(fileData)
        handle.write(Data("\r\n--\(boundary)--\r\n".utf8))
        return bodyURL
    }

    /// Writes one chunk's multipart body ({sha256, chunk_index} + bytes) to a
    /// temp file and returns its URL. The caller deletes it after the upload.
    private func writeChunkFileBody(sha256: String, chunkIndex: Int, chunkData: Data, boundary: String) throws -> URL {
        let bodyURL = FileManager.default.temporaryDirectory.appendingPathComponent("psync-chunk-\(UUID().uuidString)")
        FileManager.default.createFile(atPath: bodyURL.path, contents: nil)
        let handle = try FileHandle(forWritingTo: bodyURL)
        defer { try? handle.close() }
        handle.write(Data("--\(boundary)\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n".utf8))
        handle.write(Data("{\"sha256\":\"\(sha256)\",\"chunk_index\":\(chunkIndex)}".utf8))
        handle.write(Data("\r\n--\(boundary)\r\nContent-Disposition: form-data; name=\"file\"; filename=\"chunk\"\r\nContent-Type: application/octet-stream\r\n\r\n".utf8))
        handle.write(chunkData)
        handle.write(Data("\r\n--\(boundary)--\r\n".utf8))
        return bodyURL
    }

    /// Streams an upload request body from a file and decodes the JSON response,
    /// applying the same status-code handling as `send`. Using `upload(fromFile:)`
    /// keeps the (potentially large) body on disk instead of buffered in memory.
    private func sendUpload<T: Decodable>(_ request: URLRequest, fromFile fileURL: URL, decode: T.Type) async throws -> T {
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.upload(for: request, fromFile: fileURL)
        } catch {
            throw ApiError.transport(error.localizedDescription)
        }
        guard let http = response as? HTTPURLResponse else {
            throw ApiError.transport("no HTTP response")
        }
        if http.statusCode == 401 { throw ApiError.unauthorized }
        guard (200...299).contains(http.statusCode) else {
            let message = String(data: data, encoding: .utf8) ?? ""
            throw ApiError.server(status: http.statusCode, message: message)
        }
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw ApiError.decoding(error.localizedDescription)
        }
    }

    // MARK: - Internals

    /// Builds a JSON (or bodyless) request with optional bearer auth.
    private func makeRequest<Body: Encodable>(path: String, method: String, token: String?, jsonBody: Body?) throws -> URLRequest {
        guard let base = baseURL, let url = URL(string: path, relativeTo: base) else {
            throw ApiError.badURL
        }
        var request = URLRequest(url: url)
        request.httpMethod = method
        if let token = token {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        if let body = jsonBody {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONEncoder().encode(body)
        }
        return request
    }

    /// Assembles the multipart body: a JSON `metadata` part + a binary `file` part.
    private func makeMultipartBody(metadata: UploadMetadata, fileData: Data, boundary: String) throws -> Data {
        var body = Data()
        let metadataJSON = try JSONEncoder().encode(metadata)

        body.appendString("--\(boundary)\r\n")
        body.appendString("Content-Disposition: form-data; name=\"metadata\"\r\n")
        body.appendString("Content-Type: application/json\r\n\r\n")
        body.append(metadataJSON)
        body.appendString("\r\n")

        body.appendString("--\(boundary)\r\n")
        body.appendString("Content-Disposition: form-data; name=\"file\"; filename=\"\(metadata.filename)\"\r\n")
        body.appendString("Content-Type: \(metadata.contentType)\r\n\r\n")
        body.append(fileData)
        body.appendString("\r\n")

        body.appendString("--\(boundary)--\r\n")
        return body
    }

    /// Sends a request, maps status codes to errors, and decodes the JSON body.
    private func send<T: Decodable>(_ request: URLRequest, decode: T.Type) async throws -> T {
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: request)
        } catch {
            throw ApiError.transport(error.localizedDescription)
        }
        guard let http = response as? HTTPURLResponse else {
            throw ApiError.transport("no HTTP response")
        }
        if http.statusCode == 401 {
            throw ApiError.unauthorized
        }
        guard (200...299).contains(http.statusCode) else {
            let message = String(data: data, encoding: .utf8) ?? ""
            throw ApiError.server(status: http.statusCode, message: message)
        }
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw ApiError.decoding(error.localizedDescription)
        }
    }
}

/// Appends a UTF-8 string to a Data buffer (multipart assembly helper).
private extension Data {
    mutating func appendString(_ string: String) {
        append(Data(string.utf8))
    }
}
