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
 * Set up the createSession mock to emit a planGenerated event via the channel
 * and return a session ID.
 *
 * channel.onmessage is already set by handleGenerate() before createSession is
 * called, so calling it synchronously here is safe and avoids macrotask-timer
 * issues in the jsdom test environment.
 */
function mockCreateSessionWithPlan(planContent = "# Plan content"): void {
  vi.mocked(commands.createSession).mockImplementationOnce(
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    async (_params: any, channel: any) => {
      channel.onmessage?.({
        event: "planGenerated",
        data: { content: planContent },
      });
      return "new-sess-id";
    }
  );
}

// ─── New Session draft state persistence ─────────────────────────────────────

describe("App: New Session draft state persistence", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listSessions).mockResolvedValue([]);
    vi.mocked(commands.listConfigs).mockResolvedValue([]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("");
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
  });

  afterEach(() => {
    cleanup();
  });

  it("preserves Task input when navigating to a session and back to New Session", async () => {
    // Given: sidebar has one existing session
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-1", input: "existing task" }),
    ]);
    render(<App />);
    await waitFor(() => screen.getByText("existing task"));

    // Navigate to New Session and type a task
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    const taskTextarea = screen.getByPlaceholderText("Describe what you want to implement…");
    await userEvent.type(taskTextarea, "my draft task");

    // When: navigate to the existing session, then back to New Session
    await userEvent.click(screen.getByRole("button", { name: /existing task/ }));
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // Then: the typed task is preserved
    expect(
      screen.getByPlaceholderText("Describe what you want to implement…")
    ).toHaveValue("my draft task");
  });

  it("preserves Working Directory input when navigating away and back", async () => {
    // Given: sidebar has one existing session
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-1", input: "existing task" }),
    ]);
    render(<App />);
    await waitFor(() => screen.getByText("existing task"));

    // Navigate to New Session and type a working directory
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    const baseDirInput = screen.getByPlaceholderText("e.g. /Users/you/projects/myapp");
    await userEvent.clear(baseDirInput);
    await userEvent.type(baseDirInput, "/my/project/path");

    // When: navigate away then back
    await userEvent.click(screen.getByRole("button", { name: /existing task/ }));
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // Then: the working directory is preserved
    expect(
      screen.getByPlaceholderText("e.g. /Users/you/projects/myapp")
    ).toHaveValue("/my/project/path");
  });

  it("does not overwrite user-typed Working Directory with default loaded from listSessions on remount", async () => {
    // Given: listSessions returns a session with a specific baseDir
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-1", input: "existing task", baseDir: "/from/latest/session" }),
    ]);
    render(<App />);
    await waitFor(() => screen.getByText("existing task"));

    // Navigate to New Session, type a working directory, then navigate away
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    const baseDirInput = screen.getByPlaceholderText("e.g. /Users/you/projects/myapp");
    await userEvent.clear(baseDirInput);
    await userEvent.type(baseDirInput, "/my/typed/dir");
    await userEvent.click(screen.getByRole("button", { name: /existing task/ }));

    // When: navigate back to New Session (triggers remount, listSessions fires again)
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await act(async () => { await new Promise<void>((r) => setTimeout(r, 50)); });

    // Then: the user-typed value is NOT overwritten by the listSessions default
    expect(
      screen.getByPlaceholderText("e.g. /Users/you/projects/myapp")
    ).toHaveValue("/my/typed/dir");
  });

  it("clears all draft fields after Discard", async () => {
    // Given: App shows New Session form and a plan is generated
    mockCreateSessionWithPlan();
    vi.mocked(commands.discardSession).mockResolvedValue(undefined);

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // Enter a task and generate a plan
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement…"),
      "task to be discarded"
    );
    await userEvent.click(screen.getByRole("button", { name: "Generate plan" }));
    await waitFor(() => screen.getByRole("button", { name: "Discard" }));

    // When: click Discard
    await userEvent.click(screen.getByRole("button", { name: "Discard" }));

    // Then: the Task textarea is reset to empty
    await waitFor(() => {
      expect(
        screen.getByPlaceholderText("Describe what you want to implement…")
      ).toHaveValue("");
    });
  });

  it("keeps the generated plan visible when Discard fails", async () => {
    mockCreateSessionWithPlan();
    vi.mocked(commands.discardSession).mockRejectedValue(new Error("discard failed"));

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement…"),
      "task to keep"
    );
    await userEvent.click(screen.getByRole("button", { name: "Generate plan" }));
    await waitFor(() => screen.getByRole("button", { name: "Discard" }));

    await userEvent.click(screen.getByRole("button", { name: "Discard" }));

    expect(screen.getByText("Error: discard failed")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Discard" })).toBeInTheDocument();
  });

  it("clears draft after Approve succeeds so New Session starts fresh", async () => {
    // Given: plan is generated
    mockCreateSessionWithPlan();
    vi.mocked(commands.approveSession).mockResolvedValue(undefined);
    vi.mocked(commands.getSession).mockResolvedValue(
      makeSession({ id: "new-sess-id", input: "task to approve" })
    );
    vi.mocked(commands.listSessions).mockResolvedValue([]);

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement…"),
      "task to approve"
    );
    await userEvent.click(screen.getByRole("button", { name: "Generate plan" }));
    await waitFor(() => screen.getByRole("button", { name: "Approve" }));

    // When: approve the session
    await userEvent.click(screen.getByRole("button", { name: "Approve" }));

    await waitFor(() => {
      expect(screen.getByText("new-sess-id")).toBeInTheDocument();
    });

    // Then: returning to New Session shows a fresh draft
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await waitFor(() => {
      expect(
        screen.getByPlaceholderText("Describe what you want to implement…")
      ).toHaveValue("");
    });
  });
});

// ─── WorkflowRunner tab selection persistence ─────────────────────────────────

describe("App: WorkflowRunner tab selection persistence", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listSessions).mockResolvedValue([]);
    vi.mocked(commands.listConfigs).mockResolvedValue([]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("log line 1");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("");
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
  });

  afterEach(() => {
    cleanup();
  });

  it("remembers Plan tab for Session A when switching to Session B and back", async () => {
    // Given: two sessions A and B in the sidebar
    const sessA = makeSession({ id: "sess-a", input: "task A" });
    const sessB = makeSession({ id: "sess-b", input: "task B" });
    vi.mocked(commands.listSessions).mockResolvedValue([sessA, sessB]);

    render(<App />);
    await waitFor(() => screen.getByText("task A"));

    // Select Session A and switch to Plan tab
    await userEvent.click(screen.getByRole("button", { name: /task A/ }));
    await waitFor(() => screen.getByRole("tab", { name: "Plan" }));
    await userEvent.click(screen.getByRole("tab", { name: "Plan" }));

    // Verify Plan tab is active: Info tab's "Base dir" label is not shown
    await waitFor(() => {
      expect(screen.queryByText("Base dir")).toBeNull();
    });
    expect(screen.getByText("No plan available.")).toBeInTheDocument();

    // Navigate to Session B
    await userEvent.click(screen.getByRole("button", { name: /task B/ }));
    await waitFor(() => screen.getByRole("tab", { name: "Info" }));

    // When: go back to Session A
    await userEvent.click(screen.getByRole("button", { name: /task A/ }));
    await waitFor(() => screen.getByRole("tab", { name: "Plan" }));

    // Then: Session A should still show Plan tab, not Info tab
    // "Base dir" label only appears in the Info tab
    expect(screen.queryByText("Base dir")).toBeNull();
    expect(screen.getByText("No plan available.")).toBeInTheDocument();
  });

  it("remembers Log tab for Session B when switching to Session A and back", async () => {
    // Given: two sessions A and B
    const sessA = makeSession({ id: "sess-a", input: "task A" });
    const sessB = makeSession({ id: "sess-b", input: "task B" });
    vi.mocked(commands.listSessions).mockResolvedValue([sessA, sessB]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("log line 1\nlog line 2");

    render(<App />);
    await waitFor(() => screen.getByText("task B"));

    // Select Session B and switch to Log tab
    await userEvent.click(screen.getByRole("button", { name: /task B/ }));
    await waitFor(() => screen.getByRole("tab", { name: "Log" }));
    await userEvent.click(screen.getByRole("tab", { name: "Log" }));

    // Verify Log tab is active: Info tab's "Base dir" label is not shown
    await waitFor(() => {
      expect(screen.queryByText("Base dir")).toBeNull();
    });

    // When: navigate to Session A, then back to Session B
    await userEvent.click(screen.getByRole("button", { name: /task A/ }));
    await userEvent.click(screen.getByRole("button", { name: /task B/ }));

    // Then: Session B should still show Log tab, not Info tab
    await waitFor(() => {
      expect(screen.queryByText("Base dir")).toBeNull();
    });
  });

  it("loads plan content when returning to session with remembered Plan tab", async () => {
    // Given: session A and session B
    const sessA = makeSession({ id: "sess-a", input: "task A" });
    const sessB = makeSession({ id: "sess-b", input: "task B" });
    vi.mocked(commands.listSessions).mockResolvedValue([sessA, sessB]);
    vi.mocked(commands.getSessionPlan).mockResolvedValue("# Loaded plan");

    render(<App />);
    await waitFor(() => screen.getByText("task A"));

    // Select Session A and open Plan tab (triggers initial loadPlan)
    await userEvent.click(screen.getByRole("button", { name: /task A/ }));
    await waitFor(() => screen.getByRole("tab", { name: "Plan" }));
    await userEvent.click(screen.getByRole("tab", { name: "Plan" }));
    await waitFor(() => expect(commands.getSessionPlan).toHaveBeenCalledWith("sess-a"));

    // Reset the call count to track new calls
    vi.mocked(commands.getSessionPlan).mockClear();

    // Navigate to Session B, then back to Session A
    await userEvent.click(screen.getByRole("button", { name: /task B/ }));
    await userEvent.click(screen.getByRole("button", { name: /task A/ }));

    // Then: getSessionPlan is called again to reload plan content on return
    // (Plan tab is still remembered for Session A, so lazy load triggers)
    await waitFor(() => {
      expect(commands.getSessionPlan).toHaveBeenCalledWith("sess-a");
    });
  });
});
