import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../App";
import type { Session, WorkflowEvent } from "../types";
import * as commands from "../lib/commands";

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

function makeSession(overrides: Partial<Session> = {}): Session {
  return {
    id: "session-1",
    phase: "Planned",
    configSource: "default.yaml",
    baseDir: "/home/user/project",
    input: "test task",
    createdAt: "2026-01-01T00:00:00Z",
    workspaceMode: "Worktree",
    planAvailable: true,
    skippedSteps: [],
    ...overrides,
  };
}

async function navigateToRunAll(
  sessions: Session[],
): Promise<{ onmessage: ((event: WorkflowEvent) => void) | null }> {
  vi.mocked(commands.listSessions).mockResolvedValue(sessions);

  render(<App />);
  await waitFor(() => screen.getByRole("button", { name: /run all/i }));
  await userEvent.click(screen.getByRole("button", { name: /run all/i }));

  await waitFor(() => {
    expect(commands.runAllSessions).toHaveBeenCalledTimes(1);
  });

  return vi.mocked(commands.runAllSessions).mock.calls[0][0] as {
    onmessage: ((event: WorkflowEvent) => void) | null;
  };
}

describe("App: Run All parallel state", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listConfigs).mockResolvedValue([]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("");
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
    vi.mocked(commands.runAllSessions).mockResolvedValue();
    vi.mocked(commands.respondToOption).mockResolvedValue();
    vi.mocked(commands.getAppConfig).mockResolvedValue({ runAllParallelism: 2 });
    vi.mocked(commands.updateAppConfig).mockResolvedValue();
  });

  afterEach(() => {
    cleanup();
  });

  it("counts multiple in-flight sessions in Run All progress", async () => {
    // Given: three sessions and a Run All batch configured for parallelism=2
    const channel = await navigateToRunAll([
      makeSession({ id: "s1", input: "task one" }),
      makeSession({ id: "s2", input: "task two" }),
      makeSession({ id: "s3", input: "task three" }),
    ]);

    // When: two sessions start before either finishes
    await act(async () => {
      channel.onmessage?.({ event: "runAllStarted", data: { total: 3, parallelism: 2 } });
      channel.onmessage?.({
        event: "runAllSessionStarted",
        data: { sessionId: "s1", input: "task one" },
      });
      channel.onmessage?.({
        event: "runAllSessionStarted",
        data: { sessionId: "s2", input: "task two" },
      });
    });

    // Then: progress counts both running sessions
    await waitFor(() => {
      expect(screen.getByText(/2 \/ 3 sessions/)).toBeInTheDocument();
    });
  });

  it("keeps remaining in-flight sessions in progress after one parallel session finishes", async () => {
    // Given: three sessions and two are already running in parallel
    const channel = await navigateToRunAll([
      makeSession({ id: "s1", input: "task one" }),
      makeSession({ id: "s2", input: "task two" }),
      makeSession({ id: "s3", input: "task three" }),
    ]);

    await act(async () => {
      channel.onmessage?.({ event: "runAllStarted", data: { total: 3, parallelism: 2 } });
      channel.onmessage?.({
        event: "runAllSessionStarted",
        data: { sessionId: "s1", input: "task one" },
      });
      channel.onmessage?.({
        event: "runAllSessionStarted",
        data: { sessionId: "s2", input: "task two" },
      });
    });

    // When: one session finishes but the other is still running
    await act(async () => {
      channel.onmessage?.({
        event: "runAllSessionFinished",
        data: { sessionId: "s1", input: "task one", phase: "Completed" },
      });
    });

    // Then: progress still counts one completed + one running
    await waitFor(() => {
      expect(screen.getByText(/2 \/ 3 sessions/)).toBeInTheDocument();
    });
  });

  it("updates denominator when late-added session starts beyond initial total", async () => {
    // Given: Run All starts with total=3, representing the initial snapshot
    const channel = await navigateToRunAll([
      makeSession({ id: "s1", input: "task one" }),
      makeSession({ id: "s2", input: "task two" }),
      makeSession({ id: "s3", input: "task three" }),
    ]);

    // When: batch starts with total=3, then all 3 initially-known sessions start
    await act(async () => {
      channel.onmessage?.({ event: "runAllStarted", data: { total: 3, parallelism: 2 } });
      channel.onmessage?.({
        event: "runAllSessionStarted",
        data: { sessionId: "s1", input: "task one" },
      });
      channel.onmessage?.({
        event: "runAllSessionStarted",
        data: { sessionId: "s2", input: "task two" },
      });
      channel.onmessage?.({
        event: "runAllSessionStarted",
        data: { sessionId: "s3", input: "task three" },
      });
    });

    // And: a 4th session is picked up late (beyond the initial total of 3)
    await act(async () => {
      channel.onmessage?.({
        event: "runAllSessionStarted",
        data: { sessionId: "s4", input: "task four" },
      });
    });

    // Then: progress shows 4 / 4 sessions, not the broken 4 / 3 sessions
    await waitFor(() => {
      expect(screen.getByText(/4 \/ 4 sessions/)).toBeInTheDocument();
    });
    expect(screen.queryByText(/4 \/ 3 sessions/)).not.toBeInTheDocument();
  });

  it("preserves overlapping option prompts so both requests can be answered", async () => {
    // Given: Run All is active and two sessions ask for input before either response is sent
    const channel = await navigateToRunAll([
      makeSession({ id: "s1", input: "interactive one" }),
      makeSession({ id: "s2", input: "interactive two" }),
    ]);

    await act(async () => {
      channel.onmessage?.({ event: "runAllStarted", data: { total: 2, parallelism: 2 } });
      channel.onmessage?.({
        event: "optionRequired",
        data: {
          requestId: "req-1",
          choices: [{ label: "First choice", kind: "selector", next: "first-step" }],
          plan: "# Plan 1",
        },
      });
      channel.onmessage?.({
        event: "optionRequired",
        data: {
          requestId: "req-2",
          choices: [{ label: "Second choice", kind: "selector", next: "second-step" }],
          plan: "# Plan 2",
        },
      });
    });

    // When: the first surfaced prompt (req-1 -- oldest pending) is answered
    await userEvent.click(screen.getByRole("button", { name: "First choice" }));

    // Then: the other pending request is still surfaced and answerable afterwards
    await waitFor(() => {
      expect(commands.respondToOption).toHaveBeenCalledTimes(1);
    });
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Second choice" })).toBeInTheDocument();
    });
    await userEvent.click(screen.getByRole("button", { name: "Second choice" }));

    await waitFor(() => {
      expect(commands.respondToOption).toHaveBeenCalledTimes(2);
    });
    expect(vi.mocked(commands.respondToOption).mock.calls[0][0]).toBe("req-1");
    expect(vi.mocked(commands.respondToOption).mock.calls[1][0]).toBe("req-2");
  });
});
