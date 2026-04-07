import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../App";
import type { Session, WorkflowEvent } from "../types";
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
  channel: { onmessage: ((event: WorkflowEvent) => void) | null };
  container: HTMLElement;
}> {
  vi.mocked(commands.listSessions).mockResolvedValue(sessions);
  const result = render(<App />);
  await waitFor(() => screen.getByRole("button", { name: /run all/i }));
  await userEvent.click(screen.getByRole("button", { name: /run all/i }));

  // Wait for RunAllView to call runAllSessions and capture the channel
  await waitFor(() => {
    expect(commands.runAllSessions).toHaveBeenCalledTimes(1);
  });
  const channel = vi.mocked(commands.runAllSessions).mock.calls[0][0] as {
    onmessage: ((event: WorkflowEvent) => void) | null;
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
  });

  afterEach(() => {
    cleanup();
  });

  // --- Single session happy path ----------------------------------------------

  it("displays log lines from stepStarted and workflowCompleted events", async () => {
    // Given: Run All is running
    const { channel, container } = await navigateToRunAll();

    // When: a full single-session event sequence flows
    channel.onmessage!({ event: "runAllStarted", data: { total: 1 } });
    channel.onmessage!({
      event: "runAllSessionStarted",
      data: { sessionId: "s1", input: "build feature", total: 1 },
    });
    channel.onmessage!({ event: "stepStarted", data: { step: "Write code" } });
    channel.onmessage!({
      event: "workflowCompleted",
      data: { run: 1, skipped: 0, failed: 0 },
    });
    channel.onmessage!({
      event: "runAllSessionFinished",
      data: { sessionId: "s1", input: "build feature", phase: "Completed" },
    });

    // Then: the log area contains the step and completion entries
    const logPre = getLogPre(container);
    await waitFor(() => {
      expect(logPre.textContent).toContain("Write code");
    });
    expect(logPre.textContent).toMatch(/Completed.*run: 1/);
    expect(logPre.textContent).toMatch(/skipped: 0/);
  });

  // --- Session boundary lines -------------------------------------------------

  it("shows a boundary line when each session starts", async () => {
    // Given: Run All with 2 sessions
    const { channel, container } = await navigateToRunAll([
      makeSession({ id: "s1", input: "first task" }),
      makeSession({ id: "s2", input: "second task" }),
    ]);

    // When: first session starts
    channel.onmessage!({ event: "runAllStarted", data: { total: 2 } });
    channel.onmessage!({
      event: "runAllSessionStarted",
      data: { sessionId: "s1", input: "first task", total: 1 },
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
    channel.onmessage!({ event: "runAllStarted", data: { total: 2 } });
    channel.onmessage!({
      event: "runAllSessionStarted",
      data: { sessionId: "s1", input: "task alpha", total: 1 },
    });
    channel.onmessage!({ event: "stepStarted", data: { step: "Step A" } });
    channel.onmessage!({
      event: "workflowCompleted",
      data: { run: 1, skipped: 0, failed: 0 },
    });
    channel.onmessage!({
      event: "runAllSessionFinished",
      data: { sessionId: "s1", input: "task alpha", phase: "Completed" },
    });

    // And: second session starts and runs
    channel.onmessage!({
      event: "runAllSessionStarted",
      data: { sessionId: "s2", input: "task beta", total: 1 },
    });
    channel.onmessage!({ event: "stepStarted", data: { step: "Step B" } });
    channel.onmessage!({
      event: "workflowCompleted",
      data: { run: 1, skipped: 0, failed: 0 },
    });
    channel.onmessage!({
      event: "runAllSessionFinished",
      data: { sessionId: "s2", input: "task beta", phase: "Completed" },
    });
    channel.onmessage!({ event: "runAllCompleted", data: { cancelled: 0 } });

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

  // --- workflowFailed ---------------------------------------------------------

  it("shows a failure log line on workflowFailed", async () => {
    // Given: Run All is running
    const { channel, container } = await navigateToRunAll();

    // When: session starts and then fails
    channel.onmessage!({ event: "runAllStarted", data: { total: 1 } });
    channel.onmessage!({
      event: "runAllSessionStarted",
      data: { sessionId: "s1", input: "do thing", total: 1 },
    });
    channel.onmessage!({
      event: "workflowFailed",
      data: { error: "build error: missing dependency" },
    });
    channel.onmessage!({
      event: "runAllSessionFinished",
      data: { sessionId: "s1", input: "do thing", phase: "Failed", error: "build error: missing dependency" },
    });

    // Then: the failure message appears in the log
    const logPre = getLogPre(container);
    await waitFor(() => {
      expect(logPre.textContent).toContain("Failed");
      expect(logPre.textContent).toContain("build error: missing dependency");
    });
  });

  // --- workflowCancelled ------------------------------------------------------

  it("shows a cancellation log line on workflowCancelled", async () => {
    // Given: Run All is running
    const { channel, container } = await navigateToRunAll();

    // When: session starts and is cancelled
    channel.onmessage!({ event: "runAllStarted", data: { total: 1 } });
    channel.onmessage!({
      event: "runAllSessionStarted",
      data: { sessionId: "s1", input: "do thing", total: 1 },
    });
    channel.onmessage!({ event: "workflowCancelled" });
    channel.onmessage!({
      event: "runAllSessionFinished",
      data: { sessionId: "s1", input: "do thing", phase: "Suspended" },
    });
    channel.onmessage!({ event: "runAllCompleted", data: { cancelled: 1 } });

    // Then: the cancellation indicator appears in the log
    const logPre = getLogPre(container);
    await waitFor(() => {
      expect(logPre.textContent).toMatch(/Cancelled/);
    });
  });

  // --- No duplicate completion lines ------------------------------------------

  it("does not duplicate the completion line from workflowCompleted and runAllSessionFinished", async () => {
    // Given: Run All is running
    const { channel, container } = await navigateToRunAll();

    // When: both workflowCompleted and runAllSessionFinished fire
    channel.onmessage!({ event: "runAllStarted", data: { total: 1 } });
    channel.onmessage!({
      event: "runAllSessionStarted",
      data: { sessionId: "s1", input: "do thing", total: 1 },
    });
    channel.onmessage!({
      event: "workflowCompleted",
      data: { run: 1, skipped: 0, failed: 0 },
    });
    channel.onmessage!({
      event: "runAllSessionFinished",
      data: { sessionId: "s1", input: "do thing", phase: "Completed" },
    });

    // Then: "v Completed" appears exactly once in the log
    const logPre = getLogPre(container);
    await waitFor(() => {
      expect(logPre.textContent).toMatch(/Completed/);
    });
    const completedCount = (logPre.textContent!.match(/Completed -- run:/g) ?? []).length;
    expect(completedCount).toBe(1);
  });

  // --- optionRequired preserves log -------------------------------------------

  it("preserves accumulated log lines when optionRequired fires", async () => {
    // Given: Run All is running
    const { channel, container } = await navigateToRunAll();

    // When: some steps run, then optionRequired fires
    channel.onmessage!({ event: "runAllStarted", data: { total: 1 } });
    channel.onmessage!({
      event: "runAllSessionStarted",
      data: { sessionId: "s1", input: "interactive task", total: 1 },
    });
    channel.onmessage!({ event: "stepStarted", data: { step: "Analyze code" } });
    channel.onmessage!({
      event: "optionRequired",
      data: {
        requestId: "req-1",
        choices: [{ label: "Yes", kind: "selector", next: "step2" }],
        plan: "# Plan",
      },
    });

    // Then: the log still shows previous entries
    const logPre = getLogPre(container);
    await waitFor(() => {
      expect(logPre.textContent).toContain("Analyze code");
    });
    // And: the option dialog is visible
    expect(screen.getByText("Yes")).toBeInTheDocument();
  });

  // --- Batch start and end messages -------------------------------------------

  it("shows batch start message when runAllStarted fires", async () => {
    // Given: Run All is running
    const { channel, container } = await navigateToRunAll();

    // When: runAllStarted fires
    channel.onmessage!({ event: "runAllStarted", data: { total: 3 } });

    // Then: the log area is visible and contains a start indicator
    const logPre = getLogPre(container);
    await waitFor(() => {
      expect(logPre.textContent).toMatch(/3/);
    });
  });

  it("shows batch completion summary when runAllCompleted fires", async () => {
    // Given: Run All with 2 sessions
    const { channel, container } = await navigateToRunAll([
      makeSession({ id: "s1", input: "task 1" }),
      makeSession({ id: "s2", input: "task 2" }),
    ]);

    // When: both sessions complete and batch finishes
    channel.onmessage!({ event: "runAllStarted", data: { total: 2 } });
    channel.onmessage!({
      event: "runAllSessionStarted",
      data: { sessionId: "s1", input: "task 1", total: 1 },
    });
    channel.onmessage!({
      event: "workflowCompleted",
      data: { run: 1, skipped: 0, failed: 0 },
    });
    channel.onmessage!({
      event: "runAllSessionFinished",
      data: { sessionId: "s1", input: "task 1", phase: "Completed" },
    });
    channel.onmessage!({
      event: "runAllSessionStarted",
      data: { sessionId: "s2", input: "task 2", total: 1 },
    });
    channel.onmessage!({
      event: "workflowCompleted",
      data: { run: 1, skipped: 0, failed: 0 },
    });
    channel.onmessage!({
      event: "runAllSessionFinished",
      data: { sessionId: "s2", input: "task 2", phase: "Completed" },
    });
    channel.onmessage!({ event: "runAllCompleted", data: { cancelled: 0 } });

    // Then: the log shows a batch summary
    const logPre = getLogPre(container);
    await waitFor(() => {
      // The "Done" button appears indicating batch completed
      expect(screen.getByRole("button", { name: "Done" })).toBeInTheDocument();
    });
    // Log contains entries from both sessions
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
