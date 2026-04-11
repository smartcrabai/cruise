/**
 * Tests for the GUI settings modal (parallelism configuration).
 *
 * These tests describe the expected behaviour of the settings affordance added to
 * the sidebar header (plan #7). They will fail until the modal is implemented and
 * `getAppConfig` / `updateAppConfig` are wired into App.tsx.
 *
 * Design assumptions (from plan #7):
 *   - A settings button is visible in the sidebar header near "Clean" / "Run All" / "+ New".
 *   - Clicking it opens a modal/dialog (role="dialog").
 *   - The modal contains a numeric input (role="spinbutton") for `run_all_parallelism`.
 *   - Saving calls `updateAppConfig({ runAllParallelism: N })`.
 *   - Parallelism 0 or below is rejected with an inline validation message before saving.
 *   - Saving closes the modal on success.
 *   - A close / cancel button closes the modal without calling `updateAppConfig`.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../App";
import type { Session } from "../types";
import * as commands from "../lib/commands";
import * as desktopNotifications from "../lib/desktopNotifications";
import { openSettingsModal } from "./helpers";

// ─── Module mocks ──────────────────────────────────────────────────────────────

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn().mockResolvedValue("0.0.0"),
}));

vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
  },
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("../lib/commands", () => ({
  listSessions: vi.fn(),
  listConfigs: vi.fn(),
  createSession: vi.fn(),
  approveSession: vi.fn(),
  discardSession: vi.fn(),
  getSession: vi.fn(),
  getSessionLog: vi.fn(),
  getSessionPlan: vi.fn(),
  getNewSessionHistorySummary: vi.fn().mockResolvedValue({ recentWorkingDirs: [] }),
  getNewSessionConfigDefaults: vi.fn().mockResolvedValue({
    steps: [],
    defaultSkippedSteps: [],
  }),
  listDirectory: vi.fn(),
  getUpdateReadiness: vi.fn(),
  cleanSessions: vi.fn(),
  deleteSession: vi.fn(),
  runSession: vi.fn(),
  cancelSession: vi.fn(),
  resetSession: vi.fn(),
  respondToOption: vi.fn(),
  runAllSessions: vi.fn(),
  fixSession: vi.fn(),
  askSession: vi.fn(),
  getAppConfig: vi.fn(),
  updateAppConfig: vi.fn(),
}));

vi.mock("../lib/updater", () => ({
  checkForUpdate: vi.fn().mockResolvedValue(null),
  downloadAndInstall: vi.fn(),
}));

vi.mock("../lib/desktopNotifications", () => ({
  notifyDesktop: vi.fn(),
}));

// ─── Helpers ──────────────────────────────────────────────────────────────────

function makeSession(overrides: Partial<Session> = {}): Session {
  return {
    id: "session-1",
    phase: "Planned",
    configSource: "default.yaml",
    baseDir: "/home/user/project",
    input: "test task",
    createdAt: "2026-01-01T00:00:00Z",
    workspaceMode: "Worktree",
    skippedSteps: [],
    ...overrides,
  };
}

// ─── Tests ────────────────────────────────────────────────────────────────────

describe("App: Settings modal — basic presence", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listSessions).mockResolvedValue([makeSession()]);
    vi.mocked(commands.listConfigs).mockResolvedValue([]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("");
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
    vi.mocked(commands.getAppConfig).mockResolvedValue({ runAllParallelism: 1 });
    vi.mocked(commands.updateAppConfig).mockResolvedValue();
  });

  afterEach(() => {
    cleanup();
  });

  it("settings button is present in the sidebar", async () => {
    // Given: App is rendered
    render(<App />);

    // Then: a settings button is visible in the sidebar
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /settings/i })).toBeInTheDocument();
    });
  });

  it("opens settings modal dialog when settings button is clicked", async () => {
    // Given: App is rendered
    render(<App />);
    await waitFor(() => screen.getByRole("button", { name: /settings/i }));

    // When: settings button is clicked
    await userEvent.click(screen.getByRole("button", { name: /settings/i }));

    // Then: a dialog is visible
    await waitFor(() => {
      expect(screen.getByRole("dialog")).toBeInTheDocument();
    });
  });
});

describe("App: Settings modal — config load and display", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listSessions).mockResolvedValue([]);
    vi.mocked(commands.listConfigs).mockResolvedValue([]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("");
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
    vi.mocked(commands.updateAppConfig).mockResolvedValue();
  });

  afterEach(() => {
    cleanup();
  });

  it("shows current parallelism value from getAppConfig in the input field", async () => {
    // Given: getAppConfig returns parallelism=4
    vi.mocked(commands.getAppConfig).mockResolvedValue({ runAllParallelism: 4 });

    render(<App />);
    await waitFor(() => screen.getByRole("button", { name: /settings/i }));

    // When: settings modal is opened
    await openSettingsModal();

    // Then: the numeric input shows "4" (the persisted value)
    await waitFor(() => {
      const input = screen.getByRole("spinbutton") as HTMLInputElement;
      expect(input.value).toBe("4");
    });
  });

  it("shows default parallelism of 1 when getAppConfig returns default", async () => {
    // Given: getAppConfig returns the default value (1)
    vi.mocked(commands.getAppConfig).mockResolvedValue({ runAllParallelism: 1 });

    render(<App />);
    await waitFor(() => screen.getByRole("button", { name: /settings/i }));

    // When: settings modal is opened
    await openSettingsModal();

    // Then: the input shows "1"
    await waitFor(() => {
      const input = screen.getByRole("spinbutton") as HTMLInputElement;
      expect(input.value).toBe("1");
    });
  });
});

describe("App: Settings modal — save behaviour", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listSessions).mockResolvedValue([]);
    vi.mocked(commands.listConfigs).mockResolvedValue([]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("");
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
    vi.mocked(commands.getAppConfig).mockResolvedValue({ runAllParallelism: 1 });
    vi.mocked(commands.updateAppConfig).mockResolvedValue();
  });

  afterEach(() => {
    cleanup();
  });

  it("calls updateAppConfig with the new parallelism value when saved", async () => {
    // Given: modal is open showing parallelism=1
    render(<App />);
    await waitFor(() => screen.getByRole("button", { name: /settings/i }));
    await openSettingsModal();

    // When: change value to 3 and click save
    const input = screen.getByRole("spinbutton");
    await userEvent.clear(input);
    await userEvent.type(input, "3");
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    // Then: updateAppConfig is called with the new value
    await waitFor(() => {
      expect(vi.mocked(commands.updateAppConfig)).toHaveBeenCalledWith({ runAllParallelism: 3 });
    });
  });

  it("closes the modal after a successful save", async () => {
    // Given: modal is open
    render(<App />);
    await waitFor(() => screen.getByRole("button", { name: /settings/i }));
    await openSettingsModal();

    // When: change value and save
    const input = screen.getByRole("spinbutton");
    await userEvent.clear(input);
    await userEvent.type(input, "2");
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    // Then: modal is dismissed on success
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
  });

  it("does not call updateAppConfig when cancel/close is clicked", async () => {
    // Given: modal is open
    render(<App />);
    await waitFor(() => screen.getByRole("button", { name: /settings/i }));
    await openSettingsModal();

    // When: the close/cancel button is clicked (without saving)
    const closeButton = screen.getByRole("button", { name: /close|cancel/i });
    await userEvent.click(closeButton);

    // Then: modal closes and updateAppConfig was NOT called
    await waitFor(() => {
      expect(screen.queryByRole("dialog")).toBeNull();
    });
    expect(vi.mocked(commands.updateAppConfig)).not.toHaveBeenCalled();
  });

  it("shows an error message when updateAppConfig rejects", async () => {
    // Given: updateAppConfig will fail with a server error
    vi.mocked(commands.updateAppConfig).mockRejectedValueOnce(new Error("disk write failed"));

    render(<App />);
    await waitFor(() => screen.getByRole("button", { name: /settings/i }));
    await openSettingsModal();

    // When: change value and attempt to save
    const input = screen.getByRole("spinbutton");
    await userEvent.clear(input);
    await userEvent.type(input, "2");
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    // Then: an error message is displayed in the modal
    await waitFor(() => {
      expect(screen.getByText(/disk write failed/i)).toBeInTheDocument();
    });
    // And: the dialog is still visible (not dismissed on error)
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });
});

describe("App: Settings modal — config load failure", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listSessions).mockResolvedValue([]);
    vi.mocked(commands.listConfigs).mockResolvedValue([]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("");
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
    vi.mocked(commands.updateAppConfig).mockResolvedValue();
  });

  afterEach(() => {
    cleanup();
  });

  it("fires desktop notification and does not open dialog when getAppConfig fails", async () => {
    // Given: getAppConfig rejects when the settings button is clicked
    vi.mocked(commands.getAppConfig).mockRejectedValue(new Error("config read error"));

    render(<App />);
    await waitFor(() => screen.getByRole("button", { name: /settings/i }));

    // When: the settings button is clicked
    await userEvent.click(screen.getByRole("button", { name: /settings/i }));

    // Then: desktop notification is fired (not just addToast)
    await waitFor(() => {
      expect(vi.mocked(desktopNotifications.notifyDesktop)).toHaveBeenCalledWith(
        "Cruise",
        expect.stringContaining("Failed"),
      );
    });

    // And: the settings dialog is NOT opened (error path)
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("App: Settings modal — validation", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listSessions).mockResolvedValue([]);
    vi.mocked(commands.listConfigs).mockResolvedValue([]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("");
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
    vi.mocked(commands.getAppConfig).mockResolvedValue({ runAllParallelism: 1 });
    vi.mocked(commands.updateAppConfig).mockResolvedValue();
  });

  afterEach(() => {
    cleanup();
  });

  it("shows a validation error and does not call updateAppConfig when parallelism is 0", async () => {
    // Given: modal is open
    render(<App />);
    await waitFor(() => screen.getByRole("button", { name: /settings/i }));
    await openSettingsModal();

    // When: change parallelism to 0 (invalid — must be ≥ 1) and click save
    const input = screen.getByRole("spinbutton");
    await userEvent.clear(input);
    await userEvent.type(input, "0");
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    // Then: a validation error is shown (the value 0 is explicitly rejected)
    await waitFor(() => {
      // Error message should mention the constraint (≥1) or the invalid value
      const errorText = screen.getByText(/must be|at least|≥\s*1|minimum/i);
      expect(errorText).toBeInTheDocument();
    });
    // And: updateAppConfig must NOT be called — 0 is never silently coerced
    expect(vi.mocked(commands.updateAppConfig)).not.toHaveBeenCalled();
  });

  it("shows a validation error for negative parallelism", async () => {
    // Given: modal is open
    render(<App />);
    await waitFor(() => screen.getByRole("button", { name: /settings/i }));
    await openSettingsModal();

    // When: user types a negative number (the HTML input min=1 may prevent this,
    // but we verify the save path also guards against it)
    const input = screen.getByRole("spinbutton") as HTMLInputElement;
    // Override value directly to bypass HTML min attr
    Object.defineProperty(input, "valueAsNumber", { get: () => -1, configurable: true });
    await userEvent.clear(input);
    await userEvent.type(input, "-1");
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    // Then: updateAppConfig must NOT be called for invalid values (≤ 0)
    expect(vi.mocked(commands.updateAppConfig)).not.toHaveBeenCalled();
  });

  it("accepts parallelism of 1 (minimum valid value)", async () => {
    // Given: modal is open showing a value other than 1
    vi.mocked(commands.getAppConfig).mockResolvedValue({ runAllParallelism: 4 });

    render(<App />);
    await waitFor(() => screen.getByRole("button", { name: /settings/i }));
    await openSettingsModal();

    // When: change value to 1 and save
    const input = screen.getByRole("spinbutton");
    await userEvent.clear(input);
    await userEvent.type(input, "1");
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    // Then: updateAppConfig is called with 1 — minimum is valid
    await waitFor(() => {
      expect(vi.mocked(commands.updateAppConfig)).toHaveBeenCalledWith({ runAllParallelism: 1 });
    });
  });

  it("accepts a large parallelism value", async () => {
    // Given: modal is open
    render(<App />);
    await waitFor(() => screen.getByRole("button", { name: /settings/i }));
    await openSettingsModal();

    // When: change value to 8 and save
    const input = screen.getByRole("spinbutton");
    await userEvent.clear(input);
    await userEvent.type(input, "8");
    await userEvent.click(screen.getByRole("button", { name: /save/i }));

    // Then: updateAppConfig is called with the large value
    await waitFor(() => {
      expect(vi.mocked(commands.updateAppConfig)).toHaveBeenCalledWith({ runAllParallelism: 8 });
    });
  });
});
