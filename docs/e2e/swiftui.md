# SwiftUI and Apple native UI policy

Applies to SwiftUI and other Apple-native iOS or macOS UI targets. Apply the [common UI E2E policy](common.md); do not generalize recommendations to unsupported Apple OS targets.

- **Visual interaction:** only when suitable visual tooling is absent, use Xcode MCP to interact with the actual app, save meaningful screenshots, and inspect their actual pixels. Confirm that the selected Xcode MCP supports the required app interaction and screenshots; if it does not, report visual confirmation as blocked. SwiftUI Preview alone is not visual evidence.
- **Persistent UI E2E:** only when suitable saved UI E2E tooling is absent, store XCTest with XCUIAutomation source and provide the exact repeatable `xcodebuild test` command.
- App Intents Testing may additionally cover actual intent integration; it never replaces UI E2E.
- If the necessary Xcode host, runtime, or selected capability is unavailable, report the precise blocker and complete other reachable work without claiming that capability ran.
