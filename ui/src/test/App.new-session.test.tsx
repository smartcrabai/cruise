import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../App";
import type { Session } from "../types";
import * as commands from "../lib/commands";
import * as desktopNotifications from "../lib/desktopNotifications";

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
    ...overrides,
  };
}

// --- New Session draft state persistence -------------------------------------

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
    const taskTextarea = screen.getByPlaceholderText("Describe what you want to implement...");
    await userEvent.type(taskTextarea, "my draft task");

    // When: navigate to the existing session, then back to New Session
    await userEvent.click(screen.getByRole("button", { name: /existing task/ }));
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // Then: the typed task is preserved
    expect(
      screen.getByPlaceholderText("Describe what you want to implement...")
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

});

// --- Non-blocking session creation -------------------------------------------

/**
 * Set up the createSession mock to support a two-phase emit model:
 *  1. sessionCreated fires immediately after session is persisted - the frontend
 *     should release the New Session form at this point.
 *  2. planGenerated / planFailed fire later, after the form has already been reset.
 *
 * The mock captures the channel reference and returns control handles so tests
 * can fire each event at an explicit moment.
 */
function setupTwoPhaseCreateSession(sessionId = "new-sess-id") {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  let capturedChannel: { onmessage: ((event: any) => void) | null } | null = null;
  let resolveCreate!: (id: string) => void;

  vi.mocked(commands.createSession).mockImplementationOnce(
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (_params: any, channel: any) => {
      capturedChannel = channel;
      return new Promise<string>((resolve) => {
        resolveCreate = resolve;
      });
    }
  );

  return {
    /** Emit sessionCreated - session has been persisted, plan not yet ready. */
    emitSessionCreated(): void {
      capturedChannel!.onmessage?.({ event: "sessionCreated", data: { sessionId } });
    },
    /** Emit planGenerated and resolve the pending createSession promise. */
    emitPlanGenerated(content = "# Plan content"): void {
      capturedChannel!.onmessage?.({ event: "planGenerated", data: { sessionId, content } });
      resolveCreate(sessionId);
    },
    /** Emit planFailed and resolve the pending createSession promise. */
    emitPlanFailed(error = "plan generation failed"): void {
      capturedChannel!.onmessage?.({ event: "planFailed", data: { sessionId, error } });
      resolveCreate(sessionId);
    },
  };
}

describe("App: Non-blocking session creation", () => {
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

  it("resets task input after sessionCreated, before plan generation resolves", async () => {
    // Given: createSession emits sessionCreated before planGenerated
    const control = setupTwoPhaseCreateSession("sess-early");

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "my task"
    );
    await userEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    // When: sessionCreated fires (session is persisted, plan not yet ready)
    await act(async () => {
      control.emitSessionCreated();
    });

    // Then: task input is cleared (form released before plan is ready)
    await waitFor(() => {
      expect(
        screen.getByPlaceholderText("Describe what you want to implement...")
      ).toHaveValue("");
    });

    // Cleanup: resolve the pending createSession so the test does not leak
    await act(async () => {
      control.emitPlanGenerated();
    });
  });

  it("Generate plan button is re-enabled after sessionCreated and typing a new task", async () => {
    // Given: createSession is pending after sessionCreated
    const control = setupTwoPhaseCreateSession("sess-early");

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "another task"
    );
    await userEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    // When: sessionCreated fires and the form is released (input cleared)
    await act(async () => {
      control.emitSessionCreated();
    });
    await waitFor(() => {
      expect(
        screen.getByPlaceholderText("Describe what you want to implement...")
      ).toHaveValue("");
    });

    // When: user types a new task
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "next task"
    );

    // Then: Generate plan button is enabled
    expect(screen.getByRole("button", { name: "Generate plan" })).not.toBeDisabled();

    // Cleanup
    await act(async () => {
      control.emitPlanGenerated();
    });
  });

  it("preserves baseDir after sessionCreated clears task-scoped fields", async () => {
    // Given: form has a custom Working Directory before generate is clicked
    const control = setupTwoPhaseCreateSession("sess-early");

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    const baseDirInput = screen.getByPlaceholderText("e.g. /Users/you/projects/myapp");
    await userEvent.clear(baseDirInput);
    await userEvent.type(baseDirInput, "/my/repo/path");

    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "first task"
    );
    await userEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    // When: sessionCreated fires
    await act(async () => {
      control.emitSessionCreated();
    });

    // Then: task input is cleared but baseDir is preserved for the next session
    await waitFor(() => {
      expect(
        screen.getByPlaceholderText("Describe what you want to implement...")
      ).toHaveValue("");
    });
    expect(
      screen.getByPlaceholderText("e.g. /Users/you/projects/myapp")
    ).toHaveValue("/my/repo/path");

    // Cleanup
    await act(async () => {
      control.emitPlanGenerated();
    });
  });

  it("late planFailed does not restore old task input after form was released by sessionCreated", async () => {
    // Given: sessionCreated has already reset the form
    const control = setupTwoPhaseCreateSession("sess-fail");

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "task that will fail"
    );
    await userEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    await act(async () => {
      control.emitSessionCreated();
    });
    // Verify form was released
    await waitFor(() => {
      expect(
        screen.getByPlaceholderText("Describe what you want to implement...")
      ).toHaveValue("");
    });

    // When: planFailed fires late (after form was already released)
    await act(async () => {
      control.emitPlanFailed("model error");
    });

    // Then: task input stays empty - old draft must not be restored
    expect(
      screen.getByPlaceholderText("Describe what you want to implement...")
    ).toHaveValue("");
    // And: still on the New Session form so the user can start a fresh session
    expect(screen.getByRole("button", { name: "Generate plan" })).toBeInTheDocument();
  });

  it("late planFailed triggers sidebar refresh after form was released", async () => {
    // Given: form is released by sessionCreated
    const control = setupTwoPhaseCreateSession("sess-fail");

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "task that will fail"
    );
    await userEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    await act(async () => {
      control.emitSessionCreated();
    });
    await waitFor(() => {
      expect(
        screen.getByPlaceholderText("Describe what you want to implement...")
      ).toHaveValue("");
    });

    const callsBeforePlanFailed = vi.mocked(commands.listSessions).mock.calls.length;

    // When: planFailed fires late
    await act(async () => {
      control.emitPlanFailed("model error");
    });

    // Then: sidebar is refreshed so the backend-deleted failed session disappears promptly
    await waitFor(() => {
      expect(vi.mocked(commands.listSessions).mock.calls.length).toBeGreaterThan(
        callsBeforePlanFailed
      );
    });
  });

  it("late planGenerated triggers sidebar refresh without mutating the form", async () => {
    // Given: form is released by sessionCreated; plan arrives later
    const control = setupTwoPhaseCreateSession("sess-async");

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "async task"
    );
    await userEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    // sessionCreated: form resets
    await act(async () => {
      control.emitSessionCreated();
    });
    await waitFor(() => {
      expect(
        screen.getByPlaceholderText("Describe what you want to implement...")
      ).toHaveValue("");
    });

    const callsBeforePlanGenerated = vi.mocked(commands.listSessions).mock.calls.length;

    // When: planGenerated fires late
    await act(async () => {
      control.emitPlanGenerated("# Plan content");
    });

    // Then: sidebar is refreshed so planAvailable becomes visible immediately
    await waitFor(() => {
      expect(vi.mocked(commands.listSessions).mock.calls.length).toBeGreaterThan(
        callsBeforePlanGenerated
      );
    });

    // And: form input remains clean (late event must not mutate the draft)
    expect(
      screen.getByPlaceholderText("Describe what you want to implement...")
    ).toHaveValue("");
  });

  it("sidebar is refreshed immediately after sessionCreated without waiting for plan", async () => {
    // Given
    const control = setupTwoPhaseCreateSession("sess-refresh");

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "refresh test task"
    );
    await userEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    const callsBeforeSessionCreated = vi.mocked(commands.listSessions).mock.calls.length;

    // When: sessionCreated fires (plan not yet ready)
    await act(async () => {
      control.emitSessionCreated();
    });

    // Then: sidebar refreshes immediately (explicit refresh, not relying on 3-second poll)
    await waitFor(() => {
      expect(vi.mocked(commands.listSessions).mock.calls.length).toBeGreaterThan(
        callsBeforeSessionCreated
      );
    });

    // Cleanup
    await act(async () => {
      control.emitPlanGenerated();
    });
  });
});

// --- WorkflowRunner tab selection persistence ---------------------------------

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

    // Select Session A — Plan tab is the default
    await userEvent.click(screen.getByRole("button", { name: /task A/ }));

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
    const sessA = makeSession({ id: "sess-a", input: "task A", planAvailable: true });
    const sessB = makeSession({ id: "sess-b", input: "task B" });
    vi.mocked(commands.listSessions).mockResolvedValue([sessA, sessB]);
    vi.mocked(commands.getSessionPlan).mockResolvedValue("# Loaded plan");

    render(<App />);
    await waitFor(() => screen.getByText("task A"));

    // Select Session A — Plan tab is the default and triggers initial loadPlan
    await userEvent.click(screen.getByRole("button", { name: /task A/ }));
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

// --- Approval-ready notification transitions ----------------------------------

describe("App: Approval-ready notification transitions", () => {
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

  it("emits plan-ready toast when session transitions to approval-ready after planGenerated", async () => {
    // Given: no sessions in initial sidebar, then plan becomes available
    const control = setupTwoPhaseCreateSession("sess-plan-ready");

    render(<App />);
    await waitFor(() => screen.getByRole("button", { name: "+ New" }));

    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "needs approval",
    );
    await userEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    // sessionCreated: session appears in sidebar but plan not yet ready (Planning in UI)
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-plan-ready", phase: "Awaiting Approval", planAvailable: false }),
    ]);
    await act(async () => { control.emitSessionCreated(); });
    await waitFor(() => {
      expect(screen.getByPlaceholderText("Describe what you want to implement...")).toHaveValue("");
    });

    // When: planGenerated fires → session becomes approval-ready (planAvailable: true)
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-plan-ready", phase: "Awaiting Approval", planAvailable: true }),
    ]);
    await act(async () => { control.emitPlanGenerated(); });

    // Then: plan-ready toast appears (transition detected by snapshot detector)
    await waitFor(() => expect(screen.getByText("Plan ready")).toBeInTheDocument());
  });

  it("does not emit plan-ready notification at sessionCreated when plan is not yet available", async () => {
    // Given: session becomes visible after sessionCreated but is still in Planning state
    const control = setupTwoPhaseCreateSession("sess-planning");

    render(<App />);
    await waitFor(() => screen.getByRole("button", { name: "+ New" }));

    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "planning task",
    );
    await userEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    // When: sessionCreated fires → session has planAvailable: false (Planning in UI)
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-planning", phase: "Awaiting Approval", planAvailable: false }),
    ]);
    await act(async () => { control.emitSessionCreated(); });
    await waitFor(() => {
      expect(screen.getByPlaceholderText("Describe what you want to implement...")).toHaveValue("");
    });

    // Then: no plan-ready toast (session not approval-ready yet)
    expect(screen.queryByText("Plan ready")).not.toBeInTheDocument();

    // Cleanup: resolve pending createSession
    await act(async () => { control.emitPlanFailed(); });
  });

  it("does not emit plan-ready notification for sessions already approval-ready on app startup", async () => {
    // Given: app starts with a pre-existing approval-ready session
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "existing-approved", phase: "Awaiting Approval", planAvailable: true }),
    ]);

    render(<App />);
    // Wait for initial load to complete (session should appear in sidebar)
    await waitFor(() => screen.getByText("test task"));
    await act(async () => { await new Promise<void>((r) => setTimeout(r, 20)); });

    // Then: no plan-ready toast (startup suppression: first snapshot is never notified)
    expect(screen.queryByText("Plan ready")).not.toBeInTheDocument();
    expect(vi.mocked(desktopNotifications.notifyDesktop)).not.toHaveBeenCalledWith(
      expect.anything(),
      expect.stringContaining("Plan ready"),
    );
  });
});

// --- Plan tab as default and plan-availability gating -------------------------

describe("App: Plan tab as default and plan-availability gating", () => {
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

  it("defaults to Plan tab when first opening a session with planAvailable: true", async () => {
    // Given: a session that has a plan available
    const sess = makeSession({ id: "sess-plan", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([sess]);
    vi.mocked(commands.getSessionPlan).mockResolvedValue("# My Plan");

    render(<App />);
    await waitFor(() => screen.getByText("test task"));

    // When: open the session for the first time (no remembered tab)
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    // Then: Plan tab is active by default — Info tab's "Base dir" label is not visible
    await waitFor(() => {
      expect(screen.queryByText("Base dir")).toBeNull();
    });
    // And: getSessionPlan was called because planAvailable is true
    await waitFor(() => {
      expect(commands.getSessionPlan).toHaveBeenCalledWith("sess-plan");
    });
  });

  it("does not call getSessionPlan when opening a session with planAvailable: false", async () => {
    // Given: a session whose plan is not yet ready
    const sess = makeSession({ id: "sess-no-plan", planAvailable: false });
    vi.mocked(commands.listSessions).mockResolvedValue([sess]);

    render(<App />);
    await waitFor(() => screen.getByText("test task"));

    // When: open the session (should default to Plan tab, but plan is not available)
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    // Then: Plan tab is active (no "Base dir") and shows the empty-state text
    await waitFor(() => {
      expect(screen.queryByText("Base dir")).toBeNull();
    });
    await waitFor(() => {
      expect(screen.getByText("No plan available.")).toBeInTheDocument();
    });
    // And: getSessionPlan was NOT called — plan is not available yet
    expect(commands.getSessionPlan).not.toHaveBeenCalled();
  });

  it("auto-loads plan when open session transitions from planAvailable: false to planAvailable: true", async () => {
    // Given: session starts without a plan; Plan tab is the default
    const sessV1 = makeSession({ id: "sess-late-plan", planAvailable: false });
    vi.mocked(commands.listSessions).mockResolvedValue([sessV1]);
    vi.mocked(commands.getSessionPlan).mockResolvedValue("# Late plan");

    render(<App />);
    await waitFor(() => screen.getByText("test task"));

    // Select the session — Plan tab is shown but plan is not loaded
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));
    await waitFor(() => screen.getByText("No plan available."));

    // Confirm that no plan fetch has occurred yet
    expect(commands.getSessionPlan).not.toHaveBeenCalled();

    // When: sidebar poll sees the session transition to planAvailable: true
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-late-plan", planAvailable: true }),
    ]);
    // Trigger a silent sidebar reload via visibilitychange (same mechanism as the 3s poll)
    await act(async () => {
      document.dispatchEvent(new Event("visibilitychange"));
    });

    // Then: the plan is fetched automatically without any user interaction
    await waitFor(() => {
      expect(commands.getSessionPlan).toHaveBeenCalledWith("sess-late-plan");
    });
  });

  it("remembered non-Plan tab persists over the Plan default after navigation", async () => {
    // Given: two sessions both with plans available
    const sessA = makeSession({ id: "sess-a", input: "task A", planAvailable: true });
    const sessB = makeSession({ id: "sess-b", input: "task B", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([sessA, sessB]);

    render(<App />);
    await waitFor(() => screen.getByText("task A"));

    // Open session A — Plan tab is the default
    await userEvent.click(screen.getByRole("button", { name: /task A/ }));
    await waitFor(() => screen.getByRole("tab", { name: "Plan" }));

    // Switch to Log tab for session A (overrides the default)
    await userEvent.click(screen.getByRole("tab", { name: "Log" }));
    await waitFor(() => {
      expect(screen.getByRole("tab", { name: "Log" })).toHaveAttribute("aria-selected", "true");
    });

    // Navigate to session B, then back to session A
    await userEvent.click(screen.getByRole("button", { name: /task B/ }));
    await userEvent.click(screen.getByRole("button", { name: /task A/ }));

    // Then: Log tab is still active for session A — the remembered tab wins over the default
    await waitFor(() => {
      expect(screen.getByRole("tab", { name: "Log" })).toHaveAttribute("aria-selected", "true");
    });
    expect(screen.getByRole("tab", { name: "Plan" })).toHaveAttribute("aria-selected", "false");
  });
});
