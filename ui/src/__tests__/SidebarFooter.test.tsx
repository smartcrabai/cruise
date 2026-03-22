/**
 * Tests for the SessionSidebar footer:
 *   - current version display
 *   - update check flow (2s delay, 24h interval)
 *   - download / error / dismiss state transitions
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, act, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// ─── Module mocks (hoisted by Vitest) ─────────────────────────────────────────

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn(),
}));

vi.mock("../lib/updater", () => ({
  checkForUpdate: vi.fn(),
  downloadAndInstall: vi.fn(),
}));

vi.mock("../lib/commands", () => ({
  listSessions: vi.fn().mockResolvedValue([]),
  cleanSessions: vi.fn().mockResolvedValue({ deleted: 0, skipped: 0 }),
  approveSession: vi.fn(),
  cancelSession: vi.fn(),
  createSession: vi.fn(),
  deleteSession: vi.fn(),
  discardSession: vi.fn(),
  fixSession: vi.fn(),
  getSession: vi.fn(),
  getSessionLog: vi.fn(),
  getSessionPlan: vi.fn(),
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

// ─── Imports after mocks ───────────────────────────────────────────────────────

import { getVersion } from "@tauri-apps/api/app";
import { checkForUpdate, downloadAndInstall } from "../lib/updater";
import type { Update } from "../lib/updater";
import { SessionSidebar } from "../App";

// ─── Helpers ──────────────────────────────────────────────────────────────────

function makeUpdate(version: string): Update {
  return { version } as unknown as Update;
}

const defaultProps = {
  selectedId: null as string | null,
  onSelect: vi.fn(),
  onNewSession: vi.fn(),
};

// ─── Tests: Version display ───────────────────────────────────────────────────

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

// ─── Tests: Update check ──────────────────────────────────────────────────────

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

    // Then:  version is displayed but no Update button
    await screen.findByText(/v0\.1\.21/);
    expect(screen.queryByRole("button", { name: /update/i })).toBeNull();
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
    await screen.findByText(/0\.1\.22/);
  });

  it("shows Update button when update is available", async () => {
    // Given: checkForUpdate() returns an update
    vi.mocked(checkForUpdate).mockResolvedValue(makeUpdate("0.1.22"));

    render(<SessionSidebar {...defaultProps} />);
    await act(() => vi.advanceTimersByTimeAsync(2000));

    // Then:  Update button is displayed
    await screen.findByRole("button", { name: /update/i });
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

// ─── Tests: Update flow ───────────────────────────────────────────────────────

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

  /** Renders until the Update button is visible and returns userEvent */
  async function renderWithUpdate() {
    const user = userEvent.setup({
      advanceTimers: vi.advanceTimersByTime.bind(vi),
    });
    render(<SessionSidebar {...defaultProps} />);
    await act(() => vi.advanceTimersByTimeAsync(2000));
    await screen.findByRole("button", { name: /update/i });
    return user;
  }

  it("enters downloading state when Update button is clicked", async () => {
    // Given: downloadAndInstall() stays pending (simulating in-progress)
    vi.mocked(downloadAndInstall).mockImplementation(
      () => new Promise<void>(() => {}),
    );
    const user = await renderWithUpdate();

    // When:  Update button is clicked
    await user.click(screen.getByRole("button", { name: /update/i }));

    // Then:  a display indicating downloading appears
    await screen.findByText(/downloading/i);
  });

  it("shows error message and Dismiss button on download error", async () => {
    // Given: downloadAndInstall() throws an error
    vi.mocked(downloadAndInstall).mockRejectedValue(new Error("Network error"));
    const user = await renderWithUpdate();

    // When:  Update button is clicked
    await user.click(screen.getByRole("button", { name: /update/i }));

    // Then:  error message and Dismiss button are displayed
    await screen.findByText(/network error/i);
    await screen.findByRole("button", { name: /dismiss/i });
  });

  it("resets error state when Dismiss button is clicked", async () => {
    // Given: download failure -> error state
    vi.mocked(downloadAndInstall).mockRejectedValue(new Error("Network error"));
    const user = await renderWithUpdate();
    await user.click(screen.getByRole("button", { name: /update/i }));
    await screen.findByRole("button", { name: /dismiss/i });

    // When:  Dismiss is clicked
    await user.click(screen.getByRole("button", { name: /dismiss/i }));

    // Then:  error message disappears
    await waitFor(() => {
      expect(screen.queryByText(/network error/i)).toBeNull();
    });
  });
});
