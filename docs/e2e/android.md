# Native Android UI policy

Apply the [common UI E2E policy](common.md) to actual native Android UI flows.

- **Visual interaction:** only when appropriate existing visual tooling is absent, use mobile-mcp for actual-app interaction, saved screenshots, and pixel inspection. Confirm it supports the target app and available device/emulator.
- **Persistent E2E:** when an appropriate existing runner is absent, choose the flow-appropriate Android test: Espresso for Android Views, Compose UI Test for Compose, or UI Automator for system-UI or cross-app flows. Provide repeatable build/run commands and assertions through the real app path.
- Pin compatible versions when introducing tools. If the appropriate host, runtime, or visual/test capability is unavailable, report the exact blocker and complete other reachable work; do not silently switch frameworks or claim a pass.
