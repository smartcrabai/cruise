# React Native and Expo UI policy

Apply the [common UI E2E policy](common.md) to React Native mobile iOS and Android apps. React Native/Expo remains React Native/Expo when generated `ios/` files are present.

- **Visual interaction:** only when appropriate existing visual tooling is absent, use agent-device CLI or a connected MCP for interaction with the actual app, saved screenshots, and pixel inspection.
- **Persistent E2E:** when suitable saved UI E2E tooling is absent, store Detox + Jest source and exact repeatable `detox build` / `detox test` configurations and commands. Pin compatible versions.
- Expo requires its own native build integrated with Detox, never Expo Go. Pin and confirm compatible Expo SDK, embedded React Native, New Architecture, and Detox versions. Preserve prebuild/config-plugin-generated native workflows; do not rely on manual-only changes to generated files.
- Use iOS Simulator and a supported Android runtime as applicable; do not require blanket physical iOS device coverage. Unsupported Detox setup or compatibility is a blocker, never a reason to silently switch runners. Report exact unavailable prerequisites while completing other reachable work.
