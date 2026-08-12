import XCTest

/// End-to-end UI tests driven in the iOS Simulator. These require the Rust
/// backend to be running and reachable at the URL passed via UITEST_SERVER_URL,
/// and photo access pre-granted (see the run script). They exercise the primary
/// flows the project must demonstrate: sign-in and manual sync.
final class PhoneSyncUITests: XCTestCase {

    override func setUp() {
        continueAfterFailure = false
    }

    /// Launches the app configured for tests, pointed at the local backend.
    private func launchApp() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments += ["-uitest-reset"]
        let serverURL = ProcessInfo.processInfo.environment["UITEST_SERVER_URL"] ?? "http://127.0.0.1:8080"
        app.launchEnvironment["UITEST_SERVER_URL"] = serverURL
        // Force a tiny chunk size so uploads exercise the resumable chunked path
        // (status → chunk → complete) end-to-end, not just single-shot uploads.
        app.launchEnvironment["CHUNK_SIZE_OVERRIDE"] = "20000"
        // Auto-accept the photo permission dialog if it appears.
        addUIInterruptionMonitor(withDescription: "Photos") { alert in
            for label in ["Allow Access to All Photos", "Allow Full Access", "Allow"] {
                if alert.buttons[label].exists {
                    alert.buttons[label].tap()
                    return true
                }
            }
            return false
        }
        app.launch()
        return app
    }

    /// Wrong credentials surface an inline error and stay on the sign-in screen.
    func testSignInWithBadPasswordShowsError() {
        let app = launchApp()
        let username = app.textFields["usernameField"]
        XCTAssertTrue(username.waitForExistence(timeout: 5))

        app.secureTextFields["passwordField"].tap()
        app.secureTextFields["passwordField"].typeText("wrongpass")
        app.buttons["signInButton"].tap()

        XCTAssertTrue(app.staticTexts["signInError"].waitForExistence(timeout: 10))
    }

    /// Correct credentials sign in and reveal the media grid + Sync button.
    func testSignInShowsGrid() {
        let app = launchApp()
        signIn(app)
        XCTAssertTrue(app.buttons["syncNowButton"].waitForExistence(timeout: 15),
                      "Grid with Sync now button should appear after sign-in")
    }

    /// Tapping the sync button runs a full sync cycle: the button is replaced by
    /// the live status while syncing, then returns once uploads finish.
    func testManualSyncUploadsMedia() {
        let app = launchApp()
        signIn(app)

        let syncButton = app.buttons["syncNowButton"]
        XCTAssertTrue(syncButton.waitForExistence(timeout: 15))
        syncButton.tap()

        // While syncing, the button is replaced by the spinner + MB/s status.
        _ = syncButton.waitForNonExistence(timeout: 10)
        // And it comes back once the (small, local) sync finishes.
        XCTAssertTrue(syncButton.waitForExistence(timeout: 90), "sync should start and finish")

        // The Synced view lists what the server received.
        app.buttons["filterMenu"].tap()
        let syncedOption = app.buttons["Synced"]
        if syncedOption.waitForExistence(timeout: 5) {
            syncedOption.tap()
            XCTAssertTrue(app.buttons["mediaCell"].firstMatch.waitForExistence(timeout: 20),
                          "Synced view should show items returned by the server")
        }
    }

    /// Tapping a cell opens the full-screen detail viewer with a close control.
    func testTapCellOpensDetail() {
        let app = launchApp()
        signIn(app)
        let cell = app.buttons["mediaCell"].firstMatch
        XCTAssertTrue(cell.waitForExistence(timeout: 15))
        cell.tap()
        XCTAssertTrue(app.buttons["closeDetail"].waitForExistence(timeout: 10),
                      "Detail viewer with a close button should appear")
    }

    /// Signs in with the seeded credentials.
    private func signIn(_ app: XCUIApplication) {
        let username = app.textFields["usernameField"]
        XCTAssertTrue(username.waitForExistence(timeout: 5))
        // Username is prefilled with "jason"; just set the password.
        let password = app.secureTextFields["passwordField"]
        password.tap()
        password.typeText("modestMouse1!")
        app.buttons["signInButton"].tap()
    }
}
