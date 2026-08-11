import SwiftUI

/// Top-level view that switches between the sign-in screen and the media grid
/// based on authentication state.
struct RootView: View {
    @EnvironmentObject private var auth: AuthService

    var body: some View {
        Group {
            if auth.isSignedIn {
                MediaGridView()
            } else {
                SignInView()
            }
        }
        .animation(.default, value: auth.isSignedIn)
        .task { await demoAutoLoginIfRequested() }
    }

    /// DEBUG-only hook for automated validation: when launched with a
    /// `DEMO_PASSWORD` environment value, performs a real sign-in (same code
    /// path as the button) so an external driver can validate the running app
    /// without synthesizing keystrokes. No effect in release builds.
    private func demoAutoLoginIfRequested() async {
        #if DEBUG
        let env = ProcessInfo.processInfo.environment
        guard let password = env["DEMO_PASSWORD"], !auth.isSignedIn else { return }
        let username = env["DEMO_USER"] ?? "jason"
        try? await auth.signIn(username: username, password: password)
        #endif
    }
}
