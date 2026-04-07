import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../App";
import type { Session, SessionPhase } from "../types";
import * as commands from "../lib/commands";
import * as desktopNotifications from "../lib/desktopNotifications";

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
  getConfigSteps: vi.fn().mockResolvedValue([]),
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
 * Set up the runAllSessions mock to emit events via the captured channel.
 * Returns handles to fire each event at an explicit moment in tests.
 */
function setupRunAllSessions() {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let capturedChannel: { onmessage: ((event: any) => void) | null } | null = null;
  let resolveRunAll!: () => void;

  vi.mocked(commands.runAllSessions).mockImplementationOnce(
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (channel: any) => {
      capturedChannel = channel;
      return new Promise<void>((resolve) => {
        resolveRunAll = resolve;
      });
    }
  );

  return {
    emitRunAllStarted(total: number): void {
      capturedChannel!.onmessage?.({ event: "runAllStarted", data: { total } });
    },
    emitSessionStarted(sessionId: string, input: string): void {
      capturedChannel!.onmessage?.({
        event: "runAllSessionStarted",
        data: { sessionId, input },
      });
    },
    emitSessionFinished(sessionId: string, input: string, phase: SessionPhase, error?: string): void {
      capturedChannel!.onmessage?.({
        event: "runAllSessionFinished",
        data: { sessionId, input, phase, error },
      });
    },
    emitCompleted(cancelled = 0): void {
      capturedChannel!.onmessage?.({ event: "runAllCompleted", data: { cancelled } });
      resolveRunAll();
    },
  };
}

// ─── RunAll: sidebar refresh ──────────────────────────────────────────────────

describe("App: RunAll — sidebar refreshes immediately on session finish", () => {
  beforeEach(() => {
    vi.clearAllMocks();
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

  it("calls listSessions immediately when a RunAll session finishes, without waiting for idle poll", async () => {
    // Given: two sessions in the sidebar; runAllSessions is controlled
    const sessA = makeSession({ id: "sess-a", input: "task A", phase: "Awaiting Approval", planAvailable: true });
    const sessB = makeSession({ id: "sess-b", input: "task B", phase: "Awaiting Approval", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([sessA, sessB]);

    const control = setupRunAllSessions();

    render(<App />);
    await waitFor(() => screen.getByText("task A"));

    // Navigate to Run All view
    await userEvent.click(screen.getByRole("button", { name: /run all/i }));

    // RunAll starts; session A begins
    await act(async () => { control.emitRunAllStarted(2); });
    await act(async () => { control.emitSessionStarted("sess-a", "task A"); });

    const callsBeforeFinish = vi.mocked(commands.listSessions).mock.calls.length;

    // When: session A finishes
    await act(async () => {
      control.emitSessionFinished("sess-a", "task A", "Completed");
    });

    // Then: sidebar is refreshed immediately (not waiting for 3-second idle poll)
    await waitFor(() => {
      expect(vi.mocked(commands.listSessions).mock.calls.length).toBeGreaterThan(callsBeforeFinish);
    });

    // Cleanup
    await act(async () => { control.emitCompleted(); });
  });
});

// ─── RunAll: completed notification ──────────────────────────────────────────

describe("App: RunAll — completed notification on session finish", () => {
  beforeEach(() => {
    vi.clearAllMocks();
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

  it("emits completed notification when a RunAll session transitions to Completed", async () => {
    // Given: session starts as Awaiting Approval (plan ready)
    const session = makeSession({ id: "sess-run", input: "task run", phase: "Awaiting Approval", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);

    const control = setupRunAllSessions();

    render(<App />);
    await waitFor(() => screen.getByText("task run"));

    // Navigate to Run All
    await userEvent.click(screen.getByRole("button", { name: /run all/i }));

    await act(async () => { control.emitRunAllStarted(1); });
    await act(async () => { control.emitSessionStarted("sess-run", "task run"); });

    // When: session finishes; listSessions now returns Completed phase
    vi.mocked(commands.listSessions).mockResolvedValue([
      { ...session, phase: "Completed" },
    ]);
    await act(async () => {
      control.emitSessionFinished("sess-run", "task run", "Completed");
    });

    // Then: completed desktop notification is fired
    await waitFor(() => {
      expect(vi.mocked(desktopNotifications.notifyDesktop)).toHaveBeenCalledWith(
        "Cruise",
        expect.stringContaining("Completed"),
      );
    });

    // Cleanup
    await act(async () => { control.emitCompleted(); });
  });

  it("emits a notification for each completed session individually", async () => {
    // Given: two sessions ready to run
    const sessA = makeSession({ id: "sess-a", input: "task A", phase: "Awaiting Approval", planAvailable: true });
    const sessB = makeSession({ id: "sess-b", input: "task B", phase: "Awaiting Approval", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([sessA, sessB]);

    const control = setupRunAllSessions();

    render(<App />);
    await waitFor(() => screen.getByText("task A"));

    await userEvent.click(screen.getByRole("button", { name: /run all/i }));
    await act(async () => { control.emitRunAllStarted(2); });

    // Session A finishes
    await act(async () => { control.emitSessionStarted("sess-a", "task A"); });
    vi.mocked(commands.listSessions).mockResolvedValue([
      { ...sessA, phase: "Completed" },
      sessB,
    ]);
    await act(async () => {
      control.emitSessionFinished("sess-a", "task A", "Completed");
    });
    await waitFor(() => {
      expect(vi.mocked(desktopNotifications.notifyDesktop)).toHaveBeenCalledTimes(1);
    });

    // Session B finishes
    await act(async () => { control.emitSessionStarted("sess-b", "task B"); });
    vi.mocked(commands.listSessions).mockResolvedValue([
      { ...sessA, phase: "Completed" },
      { ...sessB, phase: "Completed" },
    ]);
    await act(async () => {
      control.emitSessionFinished("sess-b", "task B", "Completed");
    });

    // Then: two separate completed notifications fired (one per session)
    await waitFor(() => {
      expect(vi.mocked(desktopNotifications.notifyDesktop)).toHaveBeenCalledTimes(2);
    });

    // Cleanup
    await act(async () => { control.emitCompleted(); });
  });

  it("does not emit completed notification for sessions already Completed at app startup", async () => {
    // Given: app starts with sessions already in Completed state
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "pre-done", input: "pre-done task", phase: "Completed" }),
    ]);

    render(<App />);
    await waitFor(() => screen.getByText("pre-done task"));
    await act(async () => { await new Promise<void>((r) => setTimeout(r, 20)); });

    // Then: no completed notification for pre-existing completed sessions (startup suppression)
    expect(vi.mocked(desktopNotifications.notifyDesktop)).not.toHaveBeenCalled();
  });
});
