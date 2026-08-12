import SwiftUI
import Photos

/// Which subset of the library to show in the grid.
enum MediaFilter: String, CaseIterable, Identifiable {
    case all = "All"
    case unsynced = "Unsynced"
    case synced = "Synced"
    var id: String { rawValue }
}

/// The main Photos-style screen: a scrollable grid of photo/video thumbnails
/// with per-item sync badges, an All/Unsynced/Synced filter, a manual "Sync now"
/// button, and a toolbar. Tapping a cell opens the full-screen viewer.
struct MediaGridView: View {
    @EnvironmentObject private var environment: AppEnvironment
    @EnvironmentObject private var grid: GridViewModel
    @EnvironmentObject private var syncEngine: SyncEngine
    @State private var selectedAsset: PHAsset?
    @State private var showSettings = false
    @State private var filter: MediaFilter = .all

    private let columns = [GridItem(.adaptive(minimum: 110), spacing: 2)]

    /// Assets matching the active filter, using each asset's current sync state.
    private var filteredAssets: [PHAsset] {
        switch filter {
        case .all:
            return grid.assets
        case .synced:
            return grid.assets.filter { syncEngine.state(for: $0.localIdentifier) == .synced }
        case .unsynced:
            return grid.assets.filter { syncEngine.state(for: $0.localIdentifier) != .synced }
        }
    }

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("Library")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar { toolbarContent }
                .safeAreaInset(edge: .top) { filterBar }
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
            #if DEBUG
            // Debug hook: auto-start a sync on launch so a crash can be
            // reproduced/observed without tapping. Enabled via DEMO_AUTOSYNC=1.
            if ProcessInfo.processInfo.environment["DEMO_AUTOSYNC"] == "1" {
                environment.runSyncPass()
            }
            #endif
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
        } else if filteredAssets.isEmpty {
            ContentUnavailableView(
                filter == .unsynced ? "All Caught Up" : "Nothing Here",
                systemImage: filter == .unsynced ? "checkmark.circle" : "photo.on.rectangle",
                description: Text(filter == .unsynced ? "Every item has been backed up." : "No items match this filter.")
            )
        } else {
            ScrollView {
                LazyVGrid(columns: columns, spacing: 2) {
                    ForEach(filteredAssets, id: \.localIdentifier) { asset in
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

    /// Top bar: the All/Unsynced/Synced segmented filter.
    private var filterBar: some View {
        Picker("Filter", selection: $filter) {
            ForEach(MediaFilter.allCases) { Text($0.rawValue).tag($0) }
        }
        .pickerStyle(.segmented)
        .padding(.horizontal)
        .padding(.vertical, 8)
        .background(.ultraThinMaterial)
        .accessibilityIdentifier("mediaFilter")
    }

    /// Toolbar: just a settings button (the synced count lives in the sync bar,
    /// which has room to show it without truncating).
    @ToolbarContentBuilder
    private var toolbarContent: some ToolbarContent {
        ToolbarItem(placement: .topBarTrailing) {
            Button { showSettings = true } label: {
                Image(systemName: "gearshape")
            }
            .accessibilityIdentifier("settingsButton")
        }
    }

    /// Bottom bar with the synced count, progress, and the manual "Sync now"
    /// button. The count sits here (full width) so it never truncates.
    private var syncBar: some View {
        VStack(spacing: 6) {
            Text("\(syncEngine.syncedCount) of \(grid.assets.count) synced")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .accessibilityIdentifier("syncedCount")
            if syncEngine.isSyncing {
                ProgressView(value: Double(syncEngine.uploadedThisRun), total: Double(max(syncEngine.totalThisRun, 1)))
                    .padding(.horizontal)
            }
            if let error = syncEngine.lastError {
                Text(error).font(.caption).foregroundStyle(.red).lineLimit(2).multilineTextAlignment(.center)
            }
            Button {
                environment.runSyncPass()
            } label: {
                HStack(spacing: 8) {
                    if syncEngine.isSyncing {
                        // Spinning icon + live upload speed (no "Syncing…" text).
                        ProgressView().tint(.white)
                        Text(String(format: "%.1f MB/s", syncEngine.uploadSpeedMBps))
                            .fontWeight(.semibold)
                            .monospacedDigit()
                    } else {
                        Image(systemName: "arrow.triangle.2.circlepath")
                        Text("Sync now").fontWeight(.semibold)
                    }
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
