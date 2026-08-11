import SwiftUI

/// App entry point. Creates the shared environment, registers the background
/// sync task, and shows the root view.
@main
struct PhoneSyncApp: App {
    @StateObject private var environment: AppEnvironment

    /// Builds the environment and registers the background task handler before
    /// the first scene appears (required by BGTaskScheduler).
    init() {
        let env = AppEnvironment()
        _environment = StateObject(wrappedValue: env)

        BackgroundSyncScheduler.register { completion in
            env.runSyncPass { success in completion(success) }
        }
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(environment)
                .environmentObject(environment.auth)
                .environmentObject(environment.syncEngine)
                .environmentObject(environment.gridViewModel)
        }
    }
}
