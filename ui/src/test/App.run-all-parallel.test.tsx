import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup, act, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../App";
import type { Session, ApplicationEvent } from "../types";
import * as commands from "../lib/commands";
import { openSettingsModal } from "./helpers";

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
  getSessionDag: vi.fn(),
  listConfigs: vi.fn(),
  createSession: vi.fn(),
  approveSession: vi.fn(),
  discardSession: vi.fn(),
  getSession: vi.fn(),
  getSessionLog: vi.fn(),
  getSessionPlan: vi.fn(),
  getPendingPrompts: vi.fn().mockResolvedValue([]),
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
  getNewSessionHistorySummary: vi.fn().mockResolvedValue({ recentWorkingDirs: [] }),
  getNewSessionConfigDefaults: vi.fn().mockResolvedValue({
    steps: [],
    afterPrSteps: [],
    defaultSkippedSteps: [],
  }),
  getNewSessionDraft: vi.fn().mockResolvedValue(null),
  saveNewSessionDraft: vi.fn().mockResolvedValue(undefined),
  clearNewSessionDraft: vi.fn().mockResolvedValue(undefined),
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
): Promise<{ onmessage: ((event: ApplicationEvent) => void) | null }> {
  vi.mocked(commands.listSessions).mockResolvedValue(sessions);

  render(<App />);
  await waitFor(() => screen.getByRole("button", { name: "Run all pending sessions" }));
  await userEvent.click(screen.getByRole("button", { name: "Run all pending sessions" }));
  await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Run All" }));

  await waitFor(() => {
    expect(commands.runAllSessions).toHaveBeenCalledTimes(1);
  });

  return vi.mocked(commands.runAllSessions).mock.calls[0][0] as {
    onmessage: ((event: ApplicationEvent) => void) | null;
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
      channel.onmessage?.({ event: "batchStarted", data: { total: 3, parallelism: 2 } });
      channel.onmessage?.({ event: "batchSessionStarted", data: { id: "s1" } });
      channel.onmessage?.({ event: "batchSessionStarted", data: { id: "s2" } });
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
      channel.onmessage?.({ event: "batchStarted", data: { total: 3, parallelism: 2 } });
      channel.onmessage?.({ event: "batchSessionStarted", data: { id: "s1" } });
      channel.onmessage?.({ event: "batchSessionStarted", data: { id: "s2" } });
    });

    // When: one session finishes but the other is still running
    await act(async () => {
      channel.onmessage?.({ event: "batchSessionFinished", data: { id: "s1", phase: "Completed", error: null } });
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
      channel.onmessage?.({ event: "batchStarted", data: { total: 3, parallelism: 2 } });
      channel.onmessage?.({ event: "batchSessionStarted", data: { id: "s1" } });
      channel.onmessage?.({ event: "batchSessionStarted", data: { id: "s2" } });
      channel.onmessage?.({ event: "batchSessionStarted", data: { id: "s3" } });
    });

    // And: a late session changes the total before it starts
    await act(async () => {
      channel.onmessage?.({ event: "batchTotalChanged", data: { total: 4 } });
      channel.onmessage?.({ event: "batchSessionStarted", data: { id: "s4" } });
    });

    // Then: progress shows 4 / 4 sessions, not the broken 4 / 3 sessions
    await waitFor(() => {
      expect(screen.getByText(/4 \/ 4 sessions/)).toBeInTheDocument();
    });
    expect(screen.queryByText(/4 \/ 3 sessions/)).not.toBeInTheDocument();
  });

  it("preserves overlapping option prompts so both requests can be answered", async () => {
    const channel = await navigateToRunAll([
      makeSession({ id: "s1", input: "interactive one" }),
      makeSession({ id: "s2", input: "interactive two" }),
    ]);

    await act(async () => {
      channel.onmessage?.({ event: "batchStarted", data: { total: 2, parallelism: 2 } });
      channel.onmessage?.({ event: "batchSessionStarted", data: { id: "s1" } });
      channel.onmessage?.({
        event: "optionRequired",
        data: {
          sessionId: "s1",
          requestId: "req-1",
          prompt: "Choose the first step",
          choices: [{ label: "First choice", kind: "selector", nextStep: "first-step" }],
        },
      });
      channel.onmessage?.({ event: "batchSessionStarted", data: { id: "s2" } });
      channel.onmessage?.({
        event: "optionRequired",
        data: {
          sessionId: "s2",
          requestId: "req-2",
          prompt: "Choose the second step",
          choices: [{ label: "Second choice", kind: "selector", nextStep: "second-step" }],
        },
      });
    });

    await userEvent.click(screen.getByRole("button", { name: "First choice" }));
    await waitFor(() => expect(commands.respondToOption).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.getByRole("button", { name: "Second choice" })).toBeInTheDocument());
    await userEvent.click(screen.getByRole("button", { name: "Second choice" }));
    await waitFor(() => expect(commands.respondToOption).toHaveBeenCalledTimes(2));
    expect(vi.mocked(commands.respondToOption).mock.calls[0][0]).toBe("s1");
    expect(vi.mocked(commands.respondToOption).mock.calls[1][0]).toBe("s2");
  });

  it("updates parallelism denominator to new value after settings are saved while running", async () => {
    const channel = await navigateToRunAll([
      makeSession({ id: "s1", input: "task one" }),
      makeSession({ id: "s2", input: "task two" }),
    ]);

    await act(async () => {
      channel.onmessage?.({ event: "batchStarted", data: { total: 2, parallelism: 2 } });
      channel.onmessage?.({ event: "batchSessionStarted", data: { id: "s1" } });
    });

    await waitFor(() => expect(screen.getByText(/Running 1\s*\/\s*2/)).toBeInTheDocument());
    await openSettingsModal();
    const input = screen.getByRole("spinbutton");
    await userEvent.clear(input);
    await userEvent.type(input, "4");
    vi.mocked(commands.getAppConfig).mockResolvedValue({ runAllParallelism: 4 });
    await userEvent.click(screen.getByRole("button", { name: /save/i }));
    await waitFor(() => expect(screen.getByText(/Running 1\s*\/\s*4/)).toBeInTheDocument());
  });
});
