import SwiftUI

/// Reclaim device storage by deleting local copies of backed-up items. Every
/// item is re-verified against the server (exact content + full size) before
/// deletion, and iOS shows its own delete confirmation.
struct FreeUpSpaceView: View {
    @EnvironmentObject private var environment: AppEnvironment
    @EnvironmentObject private var deletionEngine: DeletionEngine
    @Environment(\.dismiss) private var dismiss

    @State private var summary: (count: Int, bytes: Int64)?
    @State private var showConfirm = false

    var body: some View {
        NavigationStack {
            List {
                summarySection
                actionSection
                if let result = deletionEngine.lastResult {
                    resultSection(result)
                }
            }
            .navigationTitle("Free Up Space")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } }
            }
            .task { if summary == nil { summary = await deletionEngine.reclaimable() } }
            .confirmationDialog(
                "Delete backed-up items from this device?",
                isPresented: $showConfirm,
                titleVisibility: .visible
            ) {
                Button("Verify & Delete", role: .destructive) {
                    Task {
                        await deletionEngine.deleteAllBackedUp()
                        summary = await deletionEngine.reclaimable()
                    }
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Each item is re-checked against the server before it's deleted. This removes it from Photos (and iCloud Photos if enabled). It stays in Recently Deleted for 30 days.")
            }
        }
    }

    private var summarySection: some View {
        Section {
            if let summary {
                LabeledContent("Backed-up on this device", value: "\(summary.count)")
                LabeledContent("Reclaimable", value: byteString(summary.bytes))
            } else {
                HStack { ProgressView(); Text("Calculating…").foregroundStyle(.secondary) }
            }
        }
    }

    private var actionSection: some View {
        Section {
            Button(role: .destructive) {
                showConfirm = true
            } label: {
                if deletionEngine.isWorking {
                    HStack {
                        ProgressView()
                        Text("Verifying \(deletionEngine.progress.checked)/\(deletionEngine.progress.total)…")
                    }
                } else {
                    Text("Delete \(summary?.count ?? 0) Items From This Device")
                }
            }
            .disabled(deletionEngine.isWorking || (summary?.count ?? 0) == 0)
            .accessibilityIdentifier("deleteBackedUpButton")
        } footer: {
            Text("Only items the server confirms it holds in full — exact content and size — are deleted. Everything remains in the Synced view.")
        }
    }

    private func resultSection(_ result: DeletionEngine.Result) -> some View {
        Section("Last Run") {
            if result.cancelled {
                Label("Cancelled — nothing was deleted.", systemImage: "xmark.circle")
            } else {
                LabeledContent("Deleted", value: "\(result.deletedCount)")
                LabeledContent("Freed", value: byteString(result.freedBytes))
            }
            if !result.kept.isEmpty {
                LabeledContent("Kept (unverified)", value: "\(result.kept.count)")
                    .foregroundStyle(.orange)
            }
        }
    }

    /// Formats a byte count for display.
    private func byteString(_ bytes: Int64) -> String {
        ByteCountFormatter.string(fromByteCount: bytes, countStyle: .file)
    }
}
