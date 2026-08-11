import Foundation

/// Handles the sign-in form's async state: submitting credentials and surfacing
/// errors, delegating the actual auth to AuthService.
@MainActor
final class SignInViewModel: ObservableObject {
    @Published var username = "jason"
    @Published var password = ""
    @Published var isSubmitting = false
    @Published var errorMessage: String?

    /// Attempts sign-in via AuthService, capturing any error for display.
    func signIn(using auth: AuthService) async {
        guard !isSubmitting else { return }
        errorMessage = nil
        isSubmitting = true
        defer { isSubmitting = false }
        do {
            try await auth.signIn(username: username, password: password)
        } catch {
            errorMessage = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }
}
