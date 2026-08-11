import Foundation

/// Owns authentication state: the stored token and server URL, sign-in, and
/// sign-out. Persists secrets in the Keychain and publishes signed-in status
/// for the UI to react to.
@MainActor
final class AuthService: ObservableObject {
    @Published private(set) var token: AuthToken?
    @Published var serverConfig: ServerConfig

    private let api: ApiClient

    private static let tokenKey = "authToken"
    private static let serverKey = "serverURL"

    /// Loads any persisted token and server URL, and configures the API client.
    /// Honors UI-test hooks: `-uitest-reset` clears the stored token, and the
    /// `UITEST_SERVER_URL` env var overrides the server address.
    init(api: ApiClient) {
        self.api = api

        let env = ProcessInfo.processInfo
        if env.arguments.contains("-uitest-reset") {
            KeychainService.remove(Self.tokenKey)
        }

        let overrideURL = env.environment["UITEST_SERVER_URL"]
        let storedURL = overrideURL ?? KeychainService.get(Self.serverKey) ?? ServerConfig.devDefault.baseURL
        self.serverConfig = ServerConfig(baseURL: storedURL)
        api.configure(baseURLString: storedURL)

        if overrideURL == nil,
           let raw = KeychainService.get(Self.tokenKey),
           let data = raw.data(using: .utf8),
           let decoded = try? JSONDecoder().decode(AuthToken.self, from: data),
           decoded.isValid {
            self.token = decoded
        }
    }

    /// True when a valid, unexpired token is held.
    var isSignedIn: Bool {
        token?.isValid == true
    }

    /// The current bearer token string, if signed in.
    var bearer: String? {
        guard let token = token, token.isValid else { return nil }
        return token.token
    }

    /// Signs in with the backend and persists the resulting token.
    func signIn(username: String, password: String) async throws {
        let response = try await api.login(username: username, password: password)
        let newToken = AuthToken(token: response.token, expiresAt: response.expiresAt)
        self.token = newToken
        if let data = try? JSONEncoder().encode(newToken), let raw = String(data: data, encoding: .utf8) {
            KeychainService.set(raw, for: Self.tokenKey)
        }
    }

    /// Clears the stored token (e.g. on 401 or user sign-out).
    func signOut() {
        token = nil
        KeychainService.remove(Self.tokenKey)
    }

    /// Updates and persists the server URL, reconfiguring the API client.
    func updateServerURL(_ urlString: String) {
        serverConfig = ServerConfig(baseURL: urlString)
        KeychainService.set(urlString, for: Self.serverKey)
        api.configure(baseURLString: urlString)
    }
}
