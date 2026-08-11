import SwiftUI
import Photos

/// The main Photos-style screen: a scrollable grid of photo/video thumbnails
/// with per-item sync badges, a manual "Sync now" button, and a toolbar. Tapping
/// a cell opens the full-screen viewer.
struct MediaGridView: View {
    @EnvironmentObject private var environment: AppEnvironment
    @EnvironmentObject private var grid: GridViewModel
    @EnvironmentObject private var syncEngine: SyncEngine
    @State private var selectedAsset: PHAsset?
    @State private var showSettings = false

    private let columns = [GridItem(.adaptive(minimum: 110), spacing: 2)]

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("Library")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar { toolbarContent }
                .safeAreaInset(edge: .bottom) { syncBar }
                .sheet(isPresented: $showSettings) { SettingsView() }
                .fullScreenCover(item: $selectedAsset) { asset in
                    MediaDetailView(asset: asset)
                }
        }
        .task {
            await grid.load()
            await syncEngine.reconcile(assets: grid.assets)
            grid.startAutoSync { environment.runSyncPass() }
        }
    }

    /// Grid of thumbnails, or an authorization-denied message.
    @ViewBuilder
    private var content: some View {
        if grid.authorizationDenied {
            ContentUnavailableView(
                "Photo Access Needed",
                systemImage: "lock.fill",
                description: Text("Enable photo access in Settings so Phone Sync can back up your media.")
            )
        } else {
            ScrollView {
                LazyVGrid(columns: columns, spacing: 2) {
                    ForEach(grid.assets, id: \.localIdentifier) { asset in
                        MediaCellView(asset: asset, state: syncEngine.state(for: asset.localIdentifier))
                            .aspectRatio(1, contentMode: .fill)
                            .onTapGesture { selectedAsset = asset }
                            .accessibilityElement(children: .combine)
                            .accessibilityAddTraits(.isButton)
                            .accessibilityIdentifier("mediaCell")
                    }
                }
                .padding(2)
            }
        }
    }

    /// Toolbar: synced count and a settings button.
    @ToolbarContentBuilder
    private var toolbarContent: some ToolbarContent {
        ToolbarItem(placement: .topBarLeading) {
            Text("\(syncEngine.syncedCount)/\(grid.assets.count) synced")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .accessibilityIdentifier("syncedCount")
        }
        ToolbarItem(placement: .topBarTrailing) {
            Button { showSettings = true } label: {
                Image(systemName: "gearshape")
            }
            .accessibilityIdentifier("settingsButton")
        }
    }

    /// Bottom bar with progress and the manual "Sync now" button.
    private var syncBar: some View {
        VStack(spacing: 6) {
            if syncEngine.isSyncing {
                ProgressView(value: Double(syncEngine.uploadedThisRun), total: Double(max(syncEngine.totalThisRun, 1)))
                    .padding(.horizontal)
            }
            if let error = syncEngine.lastError {
                Text(error).font(.caption).foregroundStyle(.red).lineLimit(2)
            }
            Button {
                environment.runSyncPass()
            } label: {
                HStack {
                    Image(systemName: "arrow.triangle.2.circlepath")
                    Text(syncEngine.isSyncing ? "Syncing… (\(syncEngine.uploadedThisRun)/\(syncEngine.totalThisRun))" : "Sync now")
                        .fontWeight(.semibold)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 12)
                .background(Color.accentColor)
                .foregroundStyle(.white)
                .clipShape(RoundedRectangle(cornerRadius: 12))
            }
            .disabled(syncEngine.isSyncing)
            .accessibilityIdentifier("syncNowButton")
            .padding(.horizontal)
            .padding(.bottom, 8)
        }
        .background(.ultraThinMaterial)
    }
}
