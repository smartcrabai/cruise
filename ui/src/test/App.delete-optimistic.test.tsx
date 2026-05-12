import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup, act, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../App";
import type { Session } from "../types";
import * as commands from "../lib/commands";

// --- Module mocks ----------------------------------------------------------------

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
  getNewSessionDraft: vi.fn().mockResolvedValue(null),
  saveNewSessionDraft: vi.fn().mockResolvedValue(undefined),
  clearNewSessionDraft: vi.fn().mockResolvedValue(undefined),
  listNewSessionHistory: vi.fn().mockResolvedValue([]),
}));

vi.mock("../lib/updater", () => ({
  checkForUpdate: vi.fn().mockResolvedValue(null),
  downloadAndInstall: vi.fn(),
}));

vi.mock("../lib/desktopNotifications", () => ({
  notifyDesktop: vi.fn(),
}));

// --- Helpers ---------------------------------------------------------------------

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

function setupBaseMocks() {
  vi.clearAllMocks();
  vi.mocked(commands.listSessions).mockResolvedValue([]);
  vi.mocked(commands.listConfigs).mockResolvedValue([]);
  vi.mocked(commands.getSessionLog).mockResolvedValue("");
  vi.mocked(commands.getSessionPlan).mockResolvedValue("");
  vi.mocked(commands.getNewSessionHistorySummary).mockResolvedValue({
    recentWorkingDirs: [],
  });
  vi.mocked(commands.getNewSessionConfigDefaults).mockResolvedValue({
    steps: [],
    defaultSkippedSteps: [],
  });
  vi.mocked(commands.listDirectory).mockResolvedValue([]);
  vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
  vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
}

async function navigateToSession(session: Session): Promise<void> {
  vi.mocked(commands.listSessions).mockResolvedValue([session]);
  render(<App />);
  await waitFor(() => screen.getByText(session.input));
  await userEvent.click(screen.getByRole("button", { name: new RegExp(session.input) }));
  await waitFor(() => screen.getByRole("button", { name: "Delete" }));
}

async function openDeleteDialog(): Promise<HTMLElement> {
  await userEvent.click(screen.getByRole("button", { name: "Delete" }));
  return screen.findByRole("dialog");
}

// --- Tests -----------------------------------------------------------------------

describe("App: Optimistic Delete", () => {
  beforeEach(setupBaseMocks);

  afterEach(() => {
    cleanup();
  });

  it("removes the session from the sidebar immediately — before the API call resolves", async () => {
    // Given: deleteSession is pending (never resolves during this test)
    let resolveDelete!: () => void;
    vi.mocked(commands.deleteSession).mockReturnValueOnce(
      new Promise<void>((r) => {
        resolveDelete = r;
      })
    );
    await navigateToSession(makeSession({ input: "test task" }));

    // When: open the delete confirmation dialog and confirm
    const dialog = await openDeleteDialog();
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));

    // Then: session disappears from the sidebar immediately (API has not resolved).
    // queryAllByText avoids the "multiple elements" error since the input text may
    // appear in both the sidebar button and the WorkflowRunner header before removal.
    expect(screen.queryAllByText("test task")).toHaveLength(0);

    // Cleanup: resolve the pending promise so no unhandled rejection remains
    await act(async () => {
      resolveDelete();
    });
  });

  it("calls deleteSession with the correct session ID", async () => {
    // Given
    vi.mocked(commands.deleteSession).mockResolvedValue(undefined);
    await navigateToSession(makeSession({ id: "sess-abc", input: "my task" }));

    // When: open dialog and confirm
    const dialog = await openDeleteDialog();
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));

    // Then
    await waitFor(() => {
      expect(commands.deleteSession).toHaveBeenCalledWith("sess-abc");
    });
  });

  it("closes the confirmation dialog immediately upon confirm — does not wait for the API", async () => {
    // Given: a slow deleteSession that does not resolve during the assertion
    let resolveDelete!: () => void;
    vi.mocked(commands.deleteSession).mockReturnValueOnce(
      new Promise<void>((r) => {
        resolveDelete = r;
      })
    );
    await navigateToSession(makeSession());
    const dialog = await openDeleteDialog();

    // When: click confirm in the dialog
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));

    // Then: dialog is gone immediately (API still pending)
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.queryByText("Deleting...")).toBeNull();

    // Cleanup
    await act(async () => {
      resolveDelete();
    });
  });

  describe("when deleteSession API call fails", () => {
    it("refreshes the sidebar to roll back the optimistic removal", async () => {
      // Given: deleteSession fails; listSessions always returns the session
      vi.mocked(commands.deleteSession).mockRejectedValue(new Error("network error"));
      const session = makeSession();
      await navigateToSession(session);

      const callCountBeforeDelete = vi.mocked(commands.listSessions).mock.calls.length;

      // When: confirm delete
      const dialog = await openDeleteDialog();
      await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));

      // Then: listSessions is called again (sidebar rollback refresh)
      await waitFor(() => {
        expect(vi.mocked(commands.listSessions).mock.calls.length).toBeGreaterThan(
          callCountBeforeDelete
        );
      });
    });

    it("shows a Failed toast", async () => {
      // Given
      vi.mocked(commands.deleteSession).mockRejectedValue(new Error("server error"));
      const session = makeSession({ input: "failing task" });
      await navigateToSession(session);

      // When: confirm delete
      const dialog = await openDeleteDialog();
      await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));

      // Then: a "Failed" toast appears
      await waitFor(() => {
        expect(screen.getByText("Failed")).toBeInTheDocument();
      });
    });
  });

  describe("when deleteSession API call succeeds", () => {
    it("shows no error toast — success is silent", async () => {
      // Given: deleteSession resolves immediately
      vi.mocked(commands.deleteSession).mockResolvedValue(undefined);
      await navigateToSession(makeSession({ input: "success task" }));

      // When: confirm delete
      const dialog = await openDeleteDialog();
      await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));

      // Wait for the delete call to have been made
      await waitFor(() => {
        expect(commands.deleteSession).toHaveBeenCalledOnce();
      });

      // Then: no "Failed" toast appears
      expect(screen.queryByText("Failed")).toBeNull();
    });
  });
});
