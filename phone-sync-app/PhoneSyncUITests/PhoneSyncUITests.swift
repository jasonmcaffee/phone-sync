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

    /// Tapping "Sync now" backs up media and advances the synced count.
    func testManualSyncUploadsMedia() {
        let app = launchApp()
        signIn(app)

        let syncButton = app.buttons["syncNowButton"]
        XCTAssertTrue(syncButton.waitForExistence(timeout: 15))
        syncButton.tap()

        // The toolbar "N/M synced" label should advance to a non-zero N once
        // uploads complete, proving media reached the server.
        let syncedCount = app.staticTexts["syncedCount"]
        XCTAssertTrue(syncedCount.waitForExistence(timeout: 5))
        let nonZeroSynced = NSPredicate(format: "NOT (label BEGINSWITH %@)", "0/")
        expectation(for: nonZeroSynced, evaluatedWith: syncedCount, handler: nil)
        waitForExpectations(timeout: 90)

        // Capture the grid with synced badges for the record.
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = "synced-grid"
        shot.lifetime = .keepAlways
        add(shot)
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
