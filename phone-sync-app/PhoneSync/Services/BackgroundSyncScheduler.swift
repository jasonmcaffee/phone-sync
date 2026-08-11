import Foundation
import BackgroundTasks

/// Registers and schedules the OS-driven background sync task. iOS cannot keep
/// an app running continuously; instead the system wakes this `BGProcessingTask`
/// opportunistically (typically on Wi-Fi/charging) to upload new media without
/// the user opening the app. Combined with the foreground library-change
/// observer, this delivers near-hands-off backup within the platform's limits.
enum BackgroundSyncScheduler {
    /// Must match a BGTaskSchedulerPermittedIdentifiers entry in Info.plist.
    static let taskIdentifier = "com.jasonmcaffee.phonesync.sync"

    /// Registers the background task handler. Call once, early in app launch.
    /// `runSync` performs one sync pass; it is given a completion to call.
    static func register(runSync: @escaping (@escaping (Bool) -> Void) -> Void) {
        BGTaskScheduler.shared.register(forTaskWithIdentifier: taskIdentifier, using: nil) { task in
            // Schedule the next run immediately so backups keep recurring.
            schedule()

            task.expirationHandler = {
                // The OS is reclaiming time; report incomplete so it retries.
                task.setTaskCompleted(success: false)
            }

            runSync { success in
                task.setTaskCompleted(success: success)
            }
        }
    }

    /// Requests the next background sync run. The OS decides actual timing.
    static func schedule() {
        let request = BGProcessingTaskRequest(identifier: taskIdentifier)
        request.requiresNetworkConnectivity = true
        request.requiresExternalPower = false
        // Earliest ~15 minutes out; the system may defer further.
        request.earliestBeginDate = Date(timeIntervalSinceNow: 15 * 60)
        do {
            try BGTaskScheduler.shared.submit(request)
        } catch {
            // Submission can fail in the simulator or when unpermitted; ignore.
            NSLog("PhoneSync: failed to schedule background sync: \(error)")
        }
    }
}
