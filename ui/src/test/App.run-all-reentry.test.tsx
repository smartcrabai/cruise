import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../App";
import type { Session } from "../types";
import * as commands from "../lib/commands";

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
    ...overrides,
  };
}

/**
 * Set up runAllSessions mock to capture the channel so tests can emit events.
 * Returns control handles for firing each event at an explicit moment.
 */
function setupRunAllMock() {
  let capturedChannel: { onmessage: ((event: unknown) => void) | null } | null = null;
  let resolveRunAll!: () => void;
  let capturedTotal = 0;

  vi.mocked(commands.runAllSessions).mockImplementationOnce(
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (channel: any) => {
      capturedChannel = channel;
      return new Promise<void>((resolve) => {
        resolveRunAll = resolve;
      });
    },
  );

  return {
    /** Emit runAllStarted event. */
    emitStarted(total: number): void {
      capturedTotal = total;
      capturedChannel!.onmessage?.({
        event: "runAllStarted",
        data: { total },
      });
    },
    /** Emit runAllSessionStarted event. */
    emitSessionStarted(sessionId: string, input: string, total?: number): void {
      capturedChannel!.onmessage?.({
        event: "runAllSessionStarted",
        data: { sessionId, input, total: total ?? capturedTotal },
      });
    },
    /** Emit runAllSessionFinished event. */
    emitSessionFinished(
      sessionId: string,
      input: string,
      phase: Session["phase"],
      error?: string,
    ): void {
      capturedChannel!.onmessage?.({
        event: "runAllSessionFinished",
        data: { sessionId, input, phase, error },
      });
    },
    /** Emit runAllCompleted event. */
    emitCompleted(cancelled = 0): void {
      capturedChannel!.onmessage?.({
        event: "runAllCompleted",
        data: { cancelled },
      });
      resolveRunAll();
    },
  };
}

// ─── Tests ────────────────────────────────────────────────────────────────────

describe("App: Run All re-entry and state persistence", () => {
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

  // --- Re-entry: navigation away and back ---

  it("returns to the same Run All progress view after navigating to a session", async () => {
    // Given: two planned sessions
    const sess1 = makeSession({ id: "s1", input: "task one" });
    const sess2 = makeSession({ id: "s2", input: "task two" });
    vi.mocked(commands.listSessions).mockResolvedValue([sess1, sess2]);
    const control = setupRunAllMock();

    render(<App />);
    await waitFor(() => screen.getByText("task one"));

    // Start Run All
    await userEvent.click(screen.getByText("Run All"));
    await act(async () => {
      control.emitStarted(2);
    });
    await act(async () => {
      control.emitSessionStarted("s1", "task one");
    });

    // Verify Run All is showing progress
    await waitFor(() => {
      expect(screen.getByText(/1 \/ 2 sessions/)).toBeInTheDocument();
    });

    // When: navigate to session detail
    await userEvent.click(screen.getByRole("button", { name: /task one/ }));

    // Then: Run All view is hidden (session detail is shown)
    await waitFor(() => {
      expect(screen.queryByText(/1 \/ 2 sessions/)).toBeNull();
    });

    // When: click Run All again (re-entry)
    await userEvent.click(screen.getByText("Run All"));

    // Then: progress is restored without a new runAllSessions call
    await waitFor(() => {
      expect(screen.getByText(/1 \/ 2 sessions/)).toBeInTheDocument();
    });
    // runAllSessions should still only have been called once
    expect(commands.runAllSessions).toHaveBeenCalledOnce();
  });

  it("does not call runAllSessions again on re-entry", async () => {
    // Given
    const sess1 = makeSession({ id: "s1", input: "task one" });
    vi.mocked(commands.listSessions).mockResolvedValue([sess1]);
    const control = setupRunAllMock();

    render(<App />);
    await waitFor(() => screen.getByText("task one"));

    // Start Run All
    await userEvent.click(screen.getByText("Run All"));
    await act(async () => {
      control.emitStarted(1);
      control.emitSessionStarted("s1", "task one");
    });

    // Navigate away then come back
    await userEvent.click(screen.getByRole("button", { name: /task one/ }));
    await userEvent.click(screen.getByText("Run All"));

    // Then: still only one call
    expect(commands.runAllSessions).toHaveBeenCalledOnce();
  });

  // --- State persistence through navigation ---

  it("preserves results when navigating away and back during execution", async () => {
    // Given
    const sess1 = makeSession({ id: "s1", input: "task one" });
    const sess2 = makeSession({ id: "s2", input: "task two" });
    vi.mocked(commands.listSessions).mockResolvedValue([sess1, sess2]);
    const control = setupRunAllMock();

    render(<App />);
    await waitFor(() => screen.getByText("task one"));

    await userEvent.click(screen.getByText("Run All"));
    await act(async () => {
      control.emitStarted(2);
      control.emitSessionStarted("s1", "task one");
      control.emitSessionFinished("s1", "task one", "Completed");
      control.emitSessionStarted("s2", "task two");
    });

    // Navigate away
    await userEvent.click(screen.getByRole("button", { name: /task one/ }));

    // Navigate back
    await userEvent.click(screen.getByText("Run All"));

    // Then: previously completed result is still visible
    await waitFor(() => {
      expect(screen.getByText(/2 \/ 2 sessions/)).toBeInTheDocument();
    });
    // The completed session result should be listed
    expect(screen.getAllByText("task one").length).toBeGreaterThan(0);
  });

  // --- Run All active button state ---

  it("shows Run All button as active (colored) during execution", async () => {
    // Given
    const sess1 = makeSession({ id: "s1", input: "task one" });
    vi.mocked(commands.listSessions).mockResolvedValue([sess1]);
    const control = setupRunAllMock();

    render(<App />);
    await waitFor(() => screen.getByText("task one"));

    // When: Run All is started
    await userEvent.click(screen.getByText("Run All"));
    await act(async () => {
      control.emitStarted(1);
    });

    // Then: Run All button has active styling
    const runAllButtons = screen.getAllByRole("button").filter(
      (btn) => btn.textContent === "Run All",
    );
    // The sidebar Run All button should have active styling
    const sidebarButton = runAllButtons[0];
    expect(sidebarButton.className).toContain("bg-blue-600");
  });

  it("enables Run All button during execution even when sessions transition to Running", async () => {
    // Given: sessions start as Planned, transition to Running after Run All starts
    const sess1 = makeSession({ id: "s1", input: "task one", phase: "Planned" });
    vi.mocked(commands.listSessions).mockResolvedValue([sess1]);
    const control = setupRunAllMock();

    render(<App />);
    await waitFor(() => screen.getByText("task one"));

    await userEvent.click(screen.getByText("Run All"));
    await act(async () => {
      control.emitStarted(1);
    });

    // When: sessions transition to Running (listSessions returns Running state)
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "s1", input: "task one", phase: "Running" }),
    ]);

    // Then: Run All button is still enabled (active state)
    // When runAllActive=true, aria-label becomes "View running sessions"
    const runAllButton = screen.getByRole("button", { name: "View running sessions" });
    expect(runAllButton).not.toBeDisabled();
  });

  // --- Completion and Done ---

  it("can return to completed results view after navigating away", async () => {
    // Given
    const sess1 = makeSession({ id: "s1", input: "task one" });
    vi.mocked(commands.listSessions).mockResolvedValue([sess1]);
    const control = setupRunAllMock();

    render(<App />);
    await waitFor(() => screen.getByText("task one"));

    await userEvent.click(screen.getByText("Run All"));
    await act(async () => {
      control.emitStarted(1);
      control.emitSessionStarted("s1", "task one");
      control.emitSessionFinished("s1", "task one", "Completed");
      control.emitCompleted();
    });

    // Navigate away
    await userEvent.click(screen.getByRole("button", { name: /task one/ }));
    await waitFor(() => {
      expect(screen.queryByText("Done")).toBeNull();
    });

    // When: click Run All to return to results
    await userEvent.click(screen.getByText("Run All"));

    // Then: completed results are shown and Done button is available
    await waitFor(() => {
      expect(screen.getByText("Done")).toBeInTheDocument();
    });
    expect(screen.getAllByText("Completed").length).toBeGreaterThan(0);
  });

  it("clears Run All state when Done is clicked, allowing fresh start", async () => {
    // Given: Run All has completed
    const sess1 = makeSession({ id: "s1", input: "task one" });
    vi.mocked(commands.listSessions).mockResolvedValue([sess1]);
    const control = setupRunAllMock();

    render(<App />);
    await waitFor(() => screen.getByText("task one"));

    await userEvent.click(screen.getByText("Run All"));
    await act(async () => {
      control.emitStarted(1);
      control.emitSessionStarted("s1", "task one");
      control.emitSessionFinished("s1", "task one", "Completed");
      control.emitCompleted();
    });

    // When: click Done
    await userEvent.click(screen.getByText("Done"));

    // Then: view returns to session list
    await waitFor(() => {
      expect(screen.queryByText("Done")).toBeNull();
    });

    // When: click Run All again — should start a new execution
    setupRunAllMock();
    await userEvent.click(screen.getByText("Run All"));

    // Then: runAllSessions is called again (second time)
    await waitFor(() => {
      expect(commands.runAllSessions).toHaveBeenCalledTimes(2);
    });
  });

  // --- Error handling ---

  it("shows error state and preserves it on re-entry", async () => {
    // Given
    const sess1 = makeSession({ id: "s1", input: "task one" });
    vi.mocked(commands.listSessions).mockResolvedValue([sess1]);

    vi.mocked(commands.runAllSessions).mockImplementationOnce(
      () => Promise.reject(new Error("backend crashed")),
    );

    render(<App />);
    await waitFor(() => screen.getByText("task one"));

    await userEvent.click(screen.getByText("Run All"));

    // Then: error is shown
    await waitFor(() => {
      expect(screen.getByText("Error")).toBeInTheDocument();
    });

    // Navigate away and come back
    await userEvent.click(screen.getByRole("button", { name: /task one/ }));
    await userEvent.click(screen.getByText("Run All"));

    // Then: error state is preserved on re-entry
    await waitFor(() => {
      expect(screen.getByText("Error")).toBeInTheDocument();
    });
  });

  // --- Cancelled state ---

  it("shows cancelled state and preserves it on re-entry", async () => {
    // Given
    const sess1 = makeSession({ id: "s1", input: "task one" });
    const sess2 = makeSession({ id: "s2", input: "task two" });
    vi.mocked(commands.listSessions).mockResolvedValue([sess1, sess2]);
    const control = setupRunAllMock();

    render(<App />);
    await waitFor(() => screen.getByText("task one"));

    await userEvent.click(screen.getByText("Run All"));
    await act(async () => {
      control.emitStarted(2);
      control.emitSessionStarted("s1", "task one");
      control.emitSessionFinished("s1", "task one", "Completed");
      // 1 cancelled
      control.emitCompleted(1);
    });

    // Then: cancelled state is shown
    await waitFor(() => {
      expect(screen.getByText("Cancelled")).toBeInTheDocument();
    });

    // Navigate away and back
    await userEvent.click(screen.getByRole("button", { name: /task one/ }));
    await userEvent.click(screen.getByText("Run All"));

    // Then: cancelled state persists
    await waitFor(() => {
      expect(screen.getByText("Cancelled")).toBeInTheDocument();
    });
  });

  // --- Sidebar refresh on Done ---

  it("triggers sidebar refresh when Done is clicked after completion", async () => {
    // Given
    const sess1 = makeSession({ id: "s1", input: "task one" });
    vi.mocked(commands.listSessions).mockResolvedValue([sess1]);
    const control = setupRunAllMock();

    render(<App />);
    await waitFor(() => {
      expect(commands.listSessions).toHaveBeenCalled();
    });

    const callsBefore = vi.mocked(commands.listSessions).mock.calls.length;

    await userEvent.click(screen.getByText("Run All"));
    await act(async () => {
      control.emitStarted(1);
      control.emitSessionStarted("s1", "task one");
      control.emitSessionFinished("s1", "task one", "Completed");
      control.emitCompleted();
    });

    // When: click Done
    await userEvent.click(screen.getByText("Done"));

    // Then: sidebar refreshes (listSessions called again)
    await waitFor(() => {
      expect(vi.mocked(commands.listSessions).mock.calls.length).toBeGreaterThan(callsBefore);
    });
  });
});
