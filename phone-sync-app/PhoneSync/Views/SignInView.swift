import SwiftUI

/// Sign-in screen. Collects credentials and the server URL, and signs in via
/// AuthService. Shown whenever there is no valid token.
struct SignInView: View {
    @EnvironmentObject private var auth: AuthService
    @StateObject private var model = SignInViewModel()
    @State private var serverURL = ""

    var body: some View {
        NavigationStack {
            VStack(spacing: 24) {
                header
                form
                signInButton
                if let error = model.errorMessage {
                    Text(error)
                        .font(.footnote)
                        .foregroundStyle(.red)
                        .multilineTextAlignment(.center)
                        .accessibilityIdentifier("signInError")
                }
                Spacer()
            }
            .padding(24)
            .navigationTitle("Phone Sync")
            .onAppear { serverURL = auth.serverConfig.baseURL }
        }
    }

    /// App icon + tagline header.
    private var header: some View {
        VStack(spacing: 8) {
            Image(systemName: "photo.on.rectangle.angled")
                .font(.system(size: 56))
                .foregroundStyle(.tint)
            Text("Back up your photos & videos")
                .font(.headline)
                .foregroundStyle(.secondary)
        }
        .padding(.top, 40)
    }

    /// Credential + server URL fields.
    private var form: some View {
        VStack(spacing: 12) {
            TextField("Username", text: $model.username)
                .textContentType(.username)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .accessibilityIdentifier("usernameField")
            SecureField("Password", text: $model.password)
                .textContentType(.password)
                .accessibilityIdentifier("passwordField")
            TextField("Server URL", text: $serverURL)
                .keyboardType(.URL)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
                .accessibilityIdentifier("serverField")
                .onChange(of: serverURL) { _, newValue in
                    auth.updateServerURL(newValue)
                }
        }
        .textFieldStyle(.roundedBorder)
    }

    /// The sign-in action button with a spinner while submitting.
    private var signInButton: some View {
        Button {
            Task { await model.signIn(using: auth) }
        } label: {
            HStack {
                if model.isSubmitting { ProgressView().tint(.white) }
                Text(model.isSubmitting ? "Signing in…" : "Sign In")
                    .fontWeight(.semibold)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 12)
            .background(Color.accentColor)
            .foregroundStyle(.white)
            .clipShape(RoundedRectangle(cornerRadius: 12))
        }
        .disabled(model.isSubmitting || model.password.isEmpty)
        .accessibilityIdentifier("signInButton")
    }
}
