/**
 * Tests for the SessionSidebar footer:
 *   - current version display
 *   - update check flow (2s delay, 24h interval)
 *   - download / error / dismiss state transitions
 *   - manual "Check Updates" button
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, act, fireEvent } from "@testing-library/react";

// --- Module mocks (hoisted by Vitest) -----------------------------------------

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn(),
}));

vi.mock("../lib/updater", () => ({
  checkForUpdate: vi.fn(),
  checkForUpdateManual: vi.fn(),
  downloadAndInstall: vi.fn(),
}));

vi.mock("../lib/commands", () => ({
  listSessions: vi.fn().mockResolvedValue([]),
  cleanSessions: vi.fn().mockResolvedValue({ deleted: 0, skipped: 0 }),
  approveSession: vi.fn(),
  cancelSession: vi.fn(),
  createSession: vi.fn(),
  deleteSession: vi.fn(),
  fixSession: vi.fn(),
  getSession: vi.fn(),
  getSessionLog: vi.fn(),
  getSessionPlan: vi.fn(),
  getUpdateReadiness: vi.fn().mockResolvedValue({ canAutoUpdate: true }),
  listConfigs: vi.fn().mockResolvedValue([]),
  listDirectory: vi.fn().mockResolvedValue([]),
  resetSession: vi.fn(),
  respondToOption: vi.fn(),
  runSession: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  Channel: vi.fn().mockImplementation(() => ({ onmessage: null })),
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

// --- Imports after mocks -------------------------------------------------------

import { getVersion } from "@tauri-apps/api/app";
import { checkForUpdate, checkForUpdateManual, downloadAndInstall } from "../lib/updater";
import type { Update } from "../lib/updater";
import { getUpdateReadiness } from "../lib/commands";
import { SessionSidebar } from "../components/SessionSidebar";

// --- Helpers ------------------------------------------------------------------

function makeUpdate(version: string): Update {
  return { version } as unknown as Update;
}

const defaultProps = {
  selectedId: null as string | null,
  onSelect: vi.fn(),
  onNewSession: vi.fn(),
  onRunAll: vi.fn(),
};

// --- Tests: Version display ---------------------------------------------------

describe("SessionSidebar footer - version display", () => {
  beforeEach(() => {
    vi.mocked(getVersion).mockResolvedValue("0.1.21");
    vi.mocked(checkForUpdate).mockResolvedValue(null);
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("displays the version number returned by getVersion() in the footer", async () => {
    // Given: getVersion() returns '0.1.21'
    // When:  SessionSidebar is mounted
    render(<SessionSidebar {...defaultProps} />);

    // Then:  'v0.1.21' is displayed in the footer
    await screen.findByText(/v0\.1\.21/);
  });
});

// --- Tests: Update check ------------------------------------------------------

describe("SessionSidebar footer - update check", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(getVersion).mockResolvedValue("0.1.21");
  });

  afterEach(() => {
    vi.clearAllMocks();
    vi.useRealTimers();
  });

  it("does not show Update button when no update is available", async () => {
    // Given: checkForUpdate() returns null
    vi.mocked(checkForUpdate).mockResolvedValue(null);

    // When:  2 seconds elapse
    render(<SessionSidebar {...defaultProps} />);
    await act(() => vi.advanceTimersByTimeAsync(2000));

    // Then:  version is displayed but no Update install button
    expect(screen.getByText(/v0\.1\.21/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Update" })).toBeNull();
  });

  it("shows new version info 2 seconds after update is available", async () => {
    // Given: checkForUpdate() returns v0.1.22
    vi.mocked(checkForUpdate).mockResolvedValue(makeUpdate("0.1.22"));

    render(<SessionSidebar {...defaultProps} />);

    // When:  update info is not shown before 2 seconds elapse
    expect(screen.queryByText(/0\.1\.22/)).toBeNull();

    // When:  after 2 seconds elapse
    await act(() => vi.advanceTimersByTimeAsync(2000));

    // Then:  new version info is displayed
    expect(screen.getByText(/0\.1\.22/)).toBeTruthy();
  });

  it("shows Update button when update is available", async () => {
    // Given: checkForUpdate() returns an update
    vi.mocked(checkForUpdate).mockResolvedValue(makeUpdate("0.1.22"));

    render(<SessionSidebar {...defaultProps} />);
    await act(() => vi.advanceTimersByTimeAsync(2000));

    // Then:  Update button is displayed
    expect(screen.getByRole("button", { name: "Update" })).toBeTruthy();
  });

  it("re-runs checkForUpdate() 24 hours after the initial check", async () => {
    // Given: initial check complete
    vi.mocked(checkForUpdate).mockResolvedValue(null);
    render(<SessionSidebar {...defaultProps} />);
    await act(() => vi.advanceTimersByTimeAsync(2000));
    expect(vi.mocked(checkForUpdate)).toHaveBeenCalledTimes(1);

    // When:  24 hours elapse
    await act(() => vi.advanceTimersByTimeAsync(24 * 60 * 60 * 1000));

    // Then:  checkForUpdate is called again
    expect(vi.mocked(checkForUpdate)).toHaveBeenCalledTimes(2);
  });
});

// --- Tests: Update flow -------------------------------------------------------

describe("SessionSidebar footer - update flow", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(getVersion).mockResolvedValue("0.1.21");
    vi.mocked(checkForUpdate).mockResolvedValue(makeUpdate("0.1.22"));
  });

  afterEach(() => {
    vi.clearAllMocks();
    vi.useRealTimers();
  });

  /** Renders until the Update button is visible */
  async function renderWithUpdate() {
    render(<SessionSidebar {...defaultProps} />);
    await act(() => vi.advanceTimersByTimeAsync(2000));
    expect(screen.getByRole("button", { name: "Update" })).toBeTruthy();
  }

  it("enters downloading state when Update button is clicked", async () => {
    // Given: downloadAndInstall() stays pending (simulating in-progress)
    vi.mocked(downloadAndInstall).mockImplementation(
      () => new Promise<void>(() => {}),
    );
    await renderWithUpdate();

    // When:  Update button is clicked
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Update" }));
    });

    // Then:  a display indicating downloading appears
    expect(screen.getByText(/downloading/i)).toBeTruthy();
  });

  it("shows error message and Dismiss button on download error", async () => {
    // Given: downloadAndInstall() throws an error
    vi.mocked(downloadAndInstall).mockRejectedValue(new Error("Network error"));
    await renderWithUpdate();

    // When:  Update button is clicked
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Update" }));
      await Promise.resolve();
    });

    // Then:  error message and Dismiss button are displayed
    expect(screen.getByText(/network error/i)).toBeTruthy();
    expect(screen.getByRole("button", { name: /dismiss/i })).toBeTruthy();
  });

  it("resets error state when Dismiss button is clicked", async () => {
    // Given: download failure -> error state
    vi.mocked(downloadAndInstall).mockRejectedValue(new Error("Network error"));
    await renderWithUpdate();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Update" }));
      await Promise.resolve();
    });
    expect(screen.getByRole("button", { name: /dismiss/i })).toBeTruthy();

    // When:  Dismiss is clicked
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /dismiss/i }));
    });

    // Then:  error message disappears
    expect(screen.queryByText(/network error/i)).toBeNull();
  });
});

// --- Tests: Update readiness guard -------------------------------------------

describe("SessionSidebar footer - update readiness guard", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(getVersion).mockResolvedValue("0.1.21");
    // An update IS available so we can confirm the button is suppressed by readiness
    vi.mocked(checkForUpdate).mockResolvedValue(makeUpdate("0.1.22"));
  });

  afterEach(() => {
    vi.clearAllMocks();
    vi.useRealTimers();
  });

  it("hides the Update button when the app is running from App Translocation", async () => {
    // Given: app is running from App Translocation (macOS Gatekeeper sandbox)
    vi.mocked(getUpdateReadiness).mockResolvedValue({
      canAutoUpdate: false,
      reason: "translocated",
      bundlePath: "/private/var/folders/xx/AppTranslocation/GUID/d/cruise.app",
      guidance: "Move cruise.app to /Applications, then run xattr -cr /Applications/cruise.app",
    });

    // When: component is mounted and 2 seconds elapse
    render(<SessionSidebar {...defaultProps} />);
    await act(() => vi.advanceTimersByTimeAsync(2000));

    // Then: Update button is not shown
    expect(screen.queryByRole("button", { name: "Update" })).toBeNull();
  });

  it("shows a warning message when the app is running from App Translocation", async () => {
    // Given: app is running from App Translocation
    vi.mocked(getUpdateReadiness).mockResolvedValue({
      canAutoUpdate: false,
      reason: "translocated",
      bundlePath: "/private/var/folders/xx/AppTranslocation/GUID/d/cruise.app",
      guidance: "Move cruise.app to /Applications, then run xattr -cr /Applications/cruise.app",
    });

    // When: component is mounted
    render(<SessionSidebar {...defaultProps} />);
    await act(() => vi.advanceTimersByTimeAsync(2000));

    // Then: guidance mentioning /Applications is displayed
    expect(screen.getByText(/\/Applications/)).toBeTruthy();
  });

  it("hides the Update button when the app is running from a mounted DMG volume", async () => {
    // Given: app is running directly from a mounted DMG
    vi.mocked(getUpdateReadiness).mockResolvedValue({
      canAutoUpdate: false,
      reason: "mountedVolume",
      bundlePath: "/Volumes/cruise 0.1.21/cruise.app",
      guidance: "Copy cruise.app to /Applications before using auto-update",
    });

    // When: component is mounted and 2 seconds elapse
    render(<SessionSidebar {...defaultProps} />);
    await act(() => vi.advanceTimersByTimeAsync(2000));

    // Then: Update button is not shown
    expect(screen.queryByRole("button", { name: "Update" })).toBeNull();
  });

  it("shows a warning message when the app is running from a mounted DMG volume", async () => {
    // Given: app is running from a mounted DMG
    vi.mocked(getUpdateReadiness).mockResolvedValue({
      canAutoUpdate: false,
      reason: "mountedVolume",
      bundlePath: "/Volumes/cruise 0.1.21/cruise.app",
      guidance: "Copy cruise.app to /Applications before using auto-update",
    });

    // When: component is mounted
    render(<SessionSidebar {...defaultProps} />);
    await act(() => vi.advanceTimersByTimeAsync(2000));

    // Then: guidance mentioning /Applications is displayed
    expect(screen.getByText(/\/Applications/)).toBeTruthy();
  });

  it("shows the Update button when readiness check returns canAutoUpdate true", async () => {
    // Given: app is properly installed (readiness OK)
    vi.mocked(getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });

    // When: component is mounted and 2 seconds elapse
    render(<SessionSidebar {...defaultProps} />);
    await act(() => vi.advanceTimersByTimeAsync(2000));

    // Then: Update button IS shown (normal behavior)
    expect(screen.getByRole("button", { name: "Update" })).toBeTruthy();
  });

  it("still displays the current version when readiness blocks auto-update", async () => {
    // Given: app is running from App Translocation
    vi.mocked(getUpdateReadiness).mockResolvedValue({
      canAutoUpdate: false,
      reason: "translocated",
      guidance: "Move cruise.app to /Applications",
    });

    // When: component is mounted
    render(<SessionSidebar {...defaultProps} />);
    await act(() => vi.advanceTimersByTimeAsync(2000));

    // Then: version number is still visible even though update is blocked
    expect(screen.getByText(/v0\.1\.21/)).toBeTruthy();
  });
});

// --- Tests: Check Updates button ---------------------------------------------

describe("SessionSidebar footer - Check Updates button", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(getVersion).mockResolvedValue("0.1.21");
    vi.mocked(checkForUpdate).mockResolvedValue(null);
    vi.mocked(checkForUpdateManual).mockResolvedValue(null);
    vi.mocked(getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
  });

  afterEach(() => {
    vi.clearAllMocks();
    vi.useRealTimers();
  });

  it("displays a 'Check Updates' button in the footer immediately on mount", async () => {
    // Given: component mounted with no auto-update timer fired yet
    // When:  SessionSidebar is rendered
    render(<SessionSidebar {...defaultProps} />);

    // Then:  Check Updates button is visible without waiting for any timer
    expect(screen.getByRole("button", { name: "Check Updates" })).toBeTruthy();
  });

  it("calls checkForUpdateManual() when Check Updates button is clicked", async () => {
    // Given: component is mounted; auto-check timer not yet fired
    render(<SessionSidebar {...defaultProps} />);

    // When:  Check Updates button is clicked
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check Updates" }));
      await Promise.resolve();
    });

    // Then:  checkForUpdateManual was called
    expect(vi.mocked(checkForUpdateManual)).toHaveBeenCalledTimes(1);
  });

  it("shows a checking indicator while the manual check is in progress", async () => {
    // Given: checkForUpdateManual() never resolves (simulating slow network)
    vi.mocked(checkForUpdateManual).mockImplementation(() => new Promise(() => {}));
    render(<SessionSidebar {...defaultProps} />);

    // When:  Check Updates button is clicked
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check Updates" }));
    });

    // Then:  a "Checking..." indicator is displayed
    expect(screen.getByText(/checking/i)).toBeTruthy();
  });

  it("disables the Check Updates button while the manual check is in progress", async () => {
    // Given: checkForUpdateManual() never resolves
    vi.mocked(checkForUpdateManual).mockImplementation(() => new Promise(() => {}));
    render(<SessionSidebar {...defaultProps} />);

    // When:  Check Updates button is clicked
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check Updates" }));
    });

    // Then:  the button is disabled to prevent double-submission
    const btn = screen.queryByRole("button", { name: "Check Updates" });
    // Either the button is removed (replaced by the checking indicator) or disabled
    if (btn) {
      expect((btn as HTMLButtonElement).disabled).toBe(true);
    } else {
      // The button was replaced by a checking indicator -- that's also acceptable
      expect(screen.getByText(/checking/i)).toBeTruthy();
    }
  });

  it("shows 'Up to date' message when manual check finds no update", async () => {
    // Given: checkForUpdateManual() returns null (no update available)
    vi.mocked(checkForUpdateManual).mockResolvedValue(null);
    render(<SessionSidebar {...defaultProps} />);

    // When:  Check Updates is clicked and resolves
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check Updates" }));
      await Promise.resolve();
    });

    // Then:  a message indicating up-to-date status is displayed
    expect(screen.getByText(/up to date/i)).toBeTruthy();
  });

  it("shows version banner when manual check finds a new version", async () => {
    // Given: auto-check returned nothing; manual check finds v0.1.99
    vi.mocked(checkForUpdate).mockResolvedValue(null);
    vi.mocked(checkForUpdateManual).mockResolvedValue(makeUpdate("0.1.99"));
    render(<SessionSidebar {...defaultProps} />);
    // Let auto-check timer fire (returns null -> no banner yet)
    await act(() => vi.advanceTimersByTimeAsync(2000));
    expect(screen.queryByText(/0\.1\.99/)).toBeNull();

    // When:  Check Updates is clicked
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check Updates" }));
      await Promise.resolve();
    });

    // Then:  new version info is displayed
    expect(screen.getByText(/0\.1\.99/)).toBeTruthy();
  });

  it("shows the Update install button when manual check finds a new version and readiness is OK", async () => {
    // Given: readiness is OK; manual check finds an update
    vi.mocked(getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(checkForUpdateManual).mockResolvedValue(makeUpdate("0.1.99"));
    render(<SessionSidebar {...defaultProps} />);

    // When:  Check Updates is clicked
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check Updates" }));
      await Promise.resolve();
    });

    // Then:  Update install button appears
    expect(screen.getByRole("button", { name: "Update" })).toBeTruthy();
  });

  it("shows an error message when the manual check fails", async () => {
    // Given: checkForUpdateManual() throws (e.g. network error)
    vi.mocked(checkForUpdateManual).mockRejectedValue(new Error("Connection timeout"));
    render(<SessionSidebar {...defaultProps} />);

    // When:  Check Updates is clicked
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check Updates" }));
      await Promise.resolve();
    });

    // Then:  an error message mentioning the failure is displayed
    expect(screen.getByText(/connection timeout/i)).toBeTruthy();
  });

  it("does not show the Update install button after manual check when readiness is NG", async () => {
    // Given: readiness blocks auto-update; manual check finds an update
    vi.mocked(getUpdateReadiness).mockResolvedValue({
      canAutoUpdate: false,
      reason: "translocated",
      guidance: "Move cruise.app to /Applications",
    });
    vi.mocked(checkForUpdateManual).mockResolvedValue(makeUpdate("0.1.99"));
    render(<SessionSidebar {...defaultProps} />);

    // When:  Check Updates is clicked
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check Updates" }));
      await Promise.resolve();
    });

    // Then:  Update install button is NOT displayed (readiness guard maintained)
    expect(screen.queryByRole("button", { name: "Update" })).toBeNull();
  });

  it("still shows readiness guidance after manual check when readiness is NG", async () => {
    // Given: readiness blocks auto-update
    vi.mocked(getUpdateReadiness).mockResolvedValue({
      canAutoUpdate: false,
      reason: "translocated",
      guidance: "Move cruise.app to /Applications",
    });
    vi.mocked(checkForUpdateManual).mockResolvedValue(makeUpdate("0.1.99"));
    render(<SessionSidebar {...defaultProps} />);
    await act(() => vi.advanceTimersByTimeAsync(2000));

    // When:  Check Updates is clicked
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Check Updates" }));
      await Promise.resolve();
    });

    // Then:  guidance is still displayed
    expect(screen.getByText(/\/Applications/)).toBeTruthy();
  });

  it("does not interfere with the existing 2-second auto-check timer", async () => {
    // Given: auto-check is set to find an update
    vi.mocked(checkForUpdate).mockResolvedValue(makeUpdate("0.1.22"));
    render(<SessionSidebar {...defaultProps} />);

    // When:  2 seconds elapse (auto-check fires)
    await act(() => vi.advanceTimersByTimeAsync(2000));

    // Then:  auto-check result is shown, and Check Updates button is still present
    expect(screen.getByText(/0\.1\.22/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Check Updates" })).toBeTruthy();
  });
});
