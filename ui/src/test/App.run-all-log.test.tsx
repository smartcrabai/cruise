import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../App";
import type { Session, ApplicationEvent } from "../types";
import * as commands from "../lib/commands";

// --- Module mocks --------------------------------------------------------------

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
  getNewSessionHistorySummary: vi.fn().mockResolvedValue({ recentWorkingDirs: [] }),
  getNewSessionConfigDefaults: vi.fn().mockResolvedValue({
    steps: [],
    afterPrSteps: [],
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

// --- Helpers ------------------------------------------------------------------

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

/**
 * Renders the App with the given sessions and navigates to the Run All view.
 * Returns the Channel instance so tests can simulate events.
 */
async function navigateToRunAll(
  sessions: Session[] = [makeSession()],
): Promise<{
  channel: { onmessage: ((event: ApplicationEvent) => void) | null };
  container: HTMLElement;
}> {
  vi.mocked(commands.listSessions).mockResolvedValue(sessions);
  const result = render(<App />);
  await waitFor(() => screen.getByRole("button", { name: "Run all pending sessions" }));
  await userEvent.click(screen.getByRole("button", { name: "Run all pending sessions" }));
  await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Run All" }));

  // Wait for RunAllView to call runAllSessions and capture the channel
  await waitFor(() => {
    expect(commands.runAllSessions).toHaveBeenCalledTimes(1);
  });
  const channel = vi.mocked(commands.runAllSessions).mock.calls[0][0] as {
    onmessage: ((event: ApplicationEvent) => void) | null;
  };
  return { channel, container: result.container };
}

/** Find the log <pre> element inside the Run All view. */
function getLogPre(container: HTMLElement): HTMLElement {
  const pre = container.querySelector("pre");
  if (!pre) throw new Error("No <pre> element found in Run All view");
  return pre;
}

// --- Tests --------------------------------------------------------------------

describe("Run All: live log display", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listConfigs).mockResolvedValue([]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("");
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
    vi.mocked(commands.runAllSessions).mockResolvedValue();
    vi.mocked(commands.getAppConfig).mockResolvedValue({ runAllParallelism: 1 });
    vi.mocked(commands.updateAppConfig).mockResolvedValue();
  });

  afterEach(() => {
    cleanup();
  });

  // --- Single session happy path ----------------------------------------------

  it("displays log lines from stepStarted and batchSessionFinished events", async () => {
    // Given: Run All is running
    const { channel, container } = await navigateToRunAll();

    // When: a full single-session event sequence flows
    // The batch event includes the configured parallelism for concurrent display.
    channel.onmessage!({ event: "batchStarted", data: { total: 1, parallelism: 1 } });
    channel.onmessage!({
      event: "batchSessionStarted",
      data: { id: "s1" },
    });
    // stepStarted carries `sessionId` to attribute concurrent progress.
    channel.onmessage!({ event: "stepStarted", data: { sessionId: "s1", step: "Write code" } });
    // The batch completion event carries the terminal session phase.
    channel.onmessage!({
      event: "batchSessionFinished",
      data: { id: "s1", phase: "Completed", error: null },
    });

    // Then: the log area contains the step and completion entries
    const logPre = getLogPre(container);
    await waitFor(() => {
      expect(logPre.textContent).toContain("Write code");
    });
    expect(logPre.textContent).toContain("[s1] Completed");
  });

  // --- Session boundary lines -------------------------------------------------

  it("shows a boundary line when each session starts", async () => {
    // Given: Run All with 2 sessions
    const { channel, container } = await navigateToRunAll([
      makeSession({ id: "s1", input: "first task" }),
      makeSession({ id: "s2", input: "second task" }),
    ]);

    // When: first session starts
    channel.onmessage!({ event: "batchStarted", data: { total: 2, parallelism: 1 } });
    channel.onmessage!({
      event: "batchSessionStarted",
      data: { id: "s1" },
    });

    // Then: log contains the first session boundary
    const logPre = getLogPre(container);
    await waitFor(() => {
      expect(logPre.textContent).toContain("first task");
    });
  });

  // --- Log accumulates across sessions ----------------------------------------

  it("accumulates log lines from multiple sessions without clearing", async () => {
    // Given: Run All with 2 sessions
    const { channel, container } = await navigateToRunAll([
      makeSession({ id: "s1", input: "task alpha" }),
      makeSession({ id: "s2", input: "task beta" }),
    ]);

    // When: first session runs to completion
    channel.onmessage!({ event: "batchStarted", data: { total: 2, parallelism: 1 } });
    channel.onmessage!({ event: "batchSessionStarted", data: { id: "s1" } });
    channel.onmessage!({ event: "stepStarted", data: { sessionId: "s1", step: "Step A" } });
    channel.onmessage!({ event: "batchSessionFinished", data: { id: "s1", phase: "Completed", error: null } });

    // And: second session starts and runs
    channel.onmessage!({ event: "batchSessionStarted", data: { id: "s2" } });
    channel.onmessage!({ event: "stepStarted", data: { sessionId: "s2", step: "Step B" } });
    channel.onmessage!({ event: "batchSessionFinished", data: { id: "s2", phase: "Completed", error: null } });
    channel.onmessage!({ event: "batchFinished", data: { cancelled: false } });

    // Then: both sessions' log lines are present
    const logPre = getLogPre(container);
    await waitFor(() => {
      expect(logPre.textContent).toContain("Step A");
      expect(logPre.textContent).toContain("Step B");
    });
    // The first session's content is not lost
    expect(logPre.textContent).toContain("task alpha");
    expect(logPre.textContent).toContain("task beta");
  });

  // --- batch session failure ---------------------------------------------------

  it("shows a failure log line on batchSessionFinished", async () => {
    const { channel, container } = await navigateToRunAll();

    channel.onmessage!({ event: "batchStarted", data: { total: 1, parallelism: 1 } });
    channel.onmessage!({ event: "batchSessionStarted", data: { id: "s1" } });
    channel.onmessage!({
      event: "batchSessionFinished",
      data: { id: "s1", phase: "Failed", error: "build error: missing dependency" },
    });

    const logPre = getLogPre(container);
    await waitFor(() => {
      expect(logPre.textContent).toContain("Failed");
      expect(logPre.textContent).toContain("build error: missing dependency");
    });
  });

  // --- batch cancellation -----------------------------------------------------

  it("shows a cancellation summary on batchFinished", async () => {
    const { channel, container } = await navigateToRunAll();

    channel.onmessage!({ event: "batchStarted", data: { total: 1, parallelism: 1 } });
    channel.onmessage!({ event: "batchSessionStarted", data: { id: "s1" } });
    channel.onmessage!({ event: "batchSessionFinished", data: { id: "s1", phase: "Suspended", error: null } });
    channel.onmessage!({ event: "batchFinished", data: { cancelled: true } });

    const logPre = getLogPre(container);
    await waitFor(() => {
      expect(logPre.textContent).toContain("cancelled: 1");
    });
  });

  // --- No duplicate completion lines ------------------------------------------

  it("shows one completion line for batchSessionFinished", async () => {
    const { channel, container } = await navigateToRunAll();

    channel.onmessage!({ event: "batchStarted", data: { total: 1, parallelism: 1 } });
    channel.onmessage!({ event: "batchSessionStarted", data: { id: "s1" } });
    channel.onmessage!({ event: "batchSessionFinished", data: { id: "s1", phase: "Completed", error: null } });

    const logPre = getLogPre(container);
    await waitFor(() => expect(logPre.textContent).toContain("[s1] Completed"));
    const completedCount = (logPre.textContent!.match(/\[s1\] Completed/g) ?? []).length;
    expect(completedCount).toBe(1);
  });

  // --- optionRequired preserves log -------------------------------------------

  it("preserves accumulated log lines when optionRequired fires", async () => {
    const { channel, container } = await navigateToRunAll();

    channel.onmessage!({ event: "batchStarted", data: { total: 1, parallelism: 1 } });
    channel.onmessage!({ event: "batchSessionStarted", data: { id: "s1" } });
    channel.onmessage!({ event: "stepStarted", data: { sessionId: "s1", step: "Analyze code" } });
    channel.onmessage!({
      event: "optionRequired",
      data: {
        sessionId: "s1",
        requestId: "req-1",
        prompt: "Choose the next step",
        choices: [{ label: "Yes", kind: "selector", nextStep: "step2" }],
      },
    });

    const logPre = getLogPre(container);
    await waitFor(() => expect(logPre.textContent).toContain("Analyze code"));
    expect(screen.getByText("Yes")).toBeInTheDocument();
  });

  // --- Batch start and end messages -------------------------------------------

  it("shows batch start message when batchStarted fires", async () => {
    const { channel, container } = await navigateToRunAll();

    channel.onmessage!({ event: "batchStarted", data: { total: 3, parallelism: 2 } });

    const logPre = getLogPre(container);
    await waitFor(() => expect(logPre.textContent).toMatch(/3/));
  });

  it("shows batch completion summary when batchFinished fires", async () => {
    const { channel, container } = await navigateToRunAll([
      makeSession({ id: "s1", input: "task 1" }),
      makeSession({ id: "s2", input: "task 2" }),
    ]);

    channel.onmessage!({ event: "batchStarted", data: { total: 2, parallelism: 1 } });
    channel.onmessage!({ event: "batchSessionStarted", data: { id: "s1" } });
    channel.onmessage!({ event: "batchSessionFinished", data: { id: "s1", phase: "Completed", error: null } });
    channel.onmessage!({ event: "batchSessionStarted", data: { id: "s2" } });
    channel.onmessage!({ event: "batchSessionFinished", data: { id: "s2", phase: "Completed", error: null } });
    channel.onmessage!({ event: "batchFinished", data: { cancelled: false } });

    const logPre = getLogPre(container);
    await waitFor(() => expect(screen.getByRole("button", { name: "Done" })).toBeInTheDocument());
    expect(logPre.textContent).toContain("task 1");
    expect(logPre.textContent).toContain("task 2");
  });

  // --- Empty log state before events ------------------------------------------

  it("shows an empty log placeholder before any events arrive", async () => {
    // Given: Run All just started
    const { channel, container } = await navigateToRunAll();

    // Then: log area shows placeholder text before events arrive
    expect(channel.onmessage).not.toBeNull();
    expect(screen.getByRole("heading", { name: "Run All" })).toBeInTheDocument();
    const logPre = getLogPre(container);
    expect(logPre.textContent).toContain("Waiting for events...");
  });
});
