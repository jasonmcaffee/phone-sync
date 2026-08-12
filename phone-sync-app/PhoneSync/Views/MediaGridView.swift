import SwiftUI
import Photos

/// Which subset of the library to show.
enum MediaFilter: String, CaseIterable, Identifiable {
    case all = "All"
    case unsynced = "Unsynced"
    case synced = "Synced"
    var id: String { rawValue }
}

/// Wrapper so the full-screen viewer can be presented at a specific index.
private struct DetailPresentation: Identifiable {
    let id = UUID()
    let items: [DisplayItem]
    let index: Int
}

/// The main screen: a Photos-style grid with a filter dropdown and grouping
/// dropdown (top-left) and the sync status/button (top-right). Shows local
/// assets for All/Unsynced and the server's own listing for Synced. Tapping a
/// cell opens a paged, zoomable, swipeable full-screen viewer.
struct MediaGridView: View {
    @EnvironmentObject private var environment: AppEnvironment
    @EnvironmentObject private var grid: GridViewModel
    @EnvironmentObject private var syncEngine: SyncEngine
    @EnvironmentObject private var auth: AuthService

    @State private var filter: MediaFilter = .all
    @State private var grouping: GroupMode = .none
    @State private var showSettings = false
    @State private var detail: DetailPresentation?

    private let columns = [GridItem(.adaptive(minimum: 110), spacing: 2)]

    /// Items for the active filter: local assets for All/Unsynced, server items
    /// for Synced (what the server actually holds).
    private var items: [DisplayItem] {
        switch filter {
        case .all:
            return grid.assets.map { .local($0) }
        case .unsynced:
            return grid.assets
                .filter { syncEngine.state(for: $0.localIdentifier) != .synced }
                .map { .local($0) }
        case .synced:
            return grid.serverItems.map { .remote($0) }
        }
    }

    private var sections: [MediaSection] { MediaGrouping.sections(items, mode: grouping) }

    var body: some View {
        NavigationStack {
            content
                .navigationBarTitleDisplayMode(.inline)
                .toolbar { toolbarContent }
                .sheet(isPresented: $showSettings) { SettingsView() }
                .fullScreenCover(item: $detail) { presentation in
                    MediaDetailView(items: presentation.items, startIndex: presentation.index)
                }
        }
        .task {
            await grid.load()
            await syncEngine.reconcile(assets: grid.assets)
            grid.startAutoSync { environment.runSyncPass() }
            #if DEBUG
            if ProcessInfo.processInfo.environment["DEMO_AUTOSYNC"] == "1" {
                environment.runSyncPass()
            }
            #endif
        }
        .onChange(of: filter) { _, newValue in
            if newValue == .synced { Task { await loadServer() } }
        }
    }

    // MARK: - Content

    @ViewBuilder
    private var content: some View {
        if grid.authorizationDenied {
            ContentUnavailableView(
                "Photo Access Needed",
                systemImage: "lock.fill",
                description: Text("Enable photo access in Settings so Phone Sync can back up your media.")
            )
        } else if filter == .synced && grid.isLoadingServer && grid.serverItems.isEmpty {
            ProgressView("Loading from server…")
        } else if items.isEmpty {
            emptyState
        } else {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 2, pinnedViews: grouping == .none ? [] : [.sectionHeaders]) {
                    ForEach(sections) { section in
                        if grouping == .none {
                            gridBody(section.items)
                        } else {
                            Section {
                                gridBody(section.items)
                            } header: {
                                Text(section.title)
                                    .font(.headline)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .padding(.horizontal, 8)
                                    .padding(.vertical, 6)
                                    .background(.ultraThinMaterial)
                            }
                        }
                    }
                }
                .padding(.bottom, 12)
            }
            .refreshable { if filter == .synced { await loadServer() } }
        }
    }

    /// A grid of cells for the given items.
    private func gridBody(_ groupItems: [DisplayItem]) -> some View {
        LazyVGrid(columns: columns, spacing: 2) {
            ForEach(groupItems) { item in
                MediaCellView(item: item, syncState: syncState(for: item))
                    .aspectRatio(1, contentMode: .fill)
                    .onTapGesture { openDetail(item) }
                    .accessibilityElement(children: .combine)
                    .accessibilityAddTraits(.isButton)
                    .accessibilityIdentifier("mediaCell")
            }
        }
        .padding(2)
    }

    /// The empty-state message for the current filter.
    private var emptyState: some View {
        ContentUnavailableView(
            filter == .unsynced ? "All Caught Up" : (filter == .synced ? "Nothing On Server" : "No Media"),
            systemImage: filter == .unsynced ? "checkmark.circle" : "photo.on.rectangle",
            description: Text(filter == .unsynced ? "Every item has been backed up." : "Nothing to show here yet.")
        )
    }

    // MARK: - Toolbar

    @ToolbarContentBuilder
    private var toolbarContent: some ToolbarContent {
        ToolbarItem(placement: .topBarLeading) { filterMenu }
        ToolbarItem(placement: .topBarLeading) { groupMenu }
        ToolbarItem(placement: .topBarTrailing) { syncStatus }
        ToolbarItem(placement: .topBarTrailing) {
            Button { showSettings = true } label: { Image(systemName: "gearshape") }
                .accessibilityIdentifier("settingsButton")
        }
    }

    /// Top-left dropdown for All / Unsynced / Synced.
    private var filterMenu: some View {
        Menu {
            Picker("Filter", selection: $filter) {
                ForEach(MediaFilter.allCases) { Text($0.rawValue).tag($0) }
            }
        } label: {
            HStack(spacing: 3) {
                Text(filter.rawValue).fontWeight(.semibold)
                Image(systemName: "chevron.down").font(.caption2)
            }
        }
        .accessibilityIdentifier("filterMenu")
    }

    /// Top-left dropdown for grouping by Day / Month / Year / None.
    private var groupMenu: some View {
        Menu {
            Picker("Group", selection: $grouping) {
                ForEach(GroupMode.allCases) { Text($0.menuLabel).tag($0) }
            }
        } label: {
            Image(systemName: "calendar")
        }
        .accessibilityIdentifier("groupMenu")
    }

    /// Top-right sync status: while syncing shows a spinner + MB/s + a yellow
    /// warning if an error is current; otherwise a tappable sync button.
    @ViewBuilder
    private var syncStatus: some View {
        if syncEngine.isSyncing {
            HStack(spacing: 5) {
                ProgressView()
                Text(String(format: "%.1f MB/s", syncEngine.uploadSpeedMBps))
                    .font(.caption).monospacedDigit()
                if syncEngine.lastError != nil {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(.yellow)
                        .accessibilityIdentifier("syncWarning")
                }
            }
        } else {
            Button { environment.runSyncPass() } label: {
                Image(systemName: "arrow.triangle.2.circlepath")
            }
            .accessibilityIdentifier("syncNowButton")
        }
    }

    // MARK: - Actions

    /// Sync state for a cell; server items are inherently synced.
    private func syncState(for item: DisplayItem) -> SyncState {
        switch item {
        case .local(let asset): return syncEngine.state(for: asset.localIdentifier)
        case .remote: return .synced
        }
    }

    /// Opens the viewer at the tapped item's position in the current list.
    private func openDetail(_ item: DisplayItem) {
        let all = items
        guard let index = all.firstIndex(where: { $0.id == item.id }) else { return }
        detail = DetailPresentation(items: all, index: index)
    }

    /// Loads the server listing (for the Synced view).
    private func loadServer() async {
        guard let token = auth.bearer else { return }
        await grid.loadServerItems(using: environment.api, token: token)
    }
}
