import SwiftUI

/// Settings sheet: edit the server URL and sign out. The server URL defaults to
/// the dev LAN address and can be pointed at https://phone.jasonmcaffee.com.
struct SettingsView: View {
    @EnvironmentObject private var auth: AuthService
    @EnvironmentObject private var syncEngine: SyncEngine
    @Environment(\.dismiss) private var dismiss

    @State private var serverURL = ""

    var body: some View {
        NavigationStack {
            Form {
                Section("Server") {
                    TextField("Server URL", text: $serverURL)
                        .keyboardType(.URL)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                        .accessibilityIdentifier("settingsServerField")
                    Button("Save") {
                        auth.updateServerURL(serverURL)
                    }
                    .accessibilityIdentifier("saveServerButton")
                }

                Section("Backup") {
                    LabeledContent("Synced items", value: "\(syncEngine.syncedCount)")
                }

                Section {
                    Button("Sign Out", role: .destructive) {
                        auth.signOut()
                        dismiss()
                    }
                    .accessibilityIdentifier("signOutButton")
                }
            }
            .navigationTitle("Settings")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                }
            }
            .onAppear { serverURL = auth.serverConfig.baseURL }
        }
    }
}
