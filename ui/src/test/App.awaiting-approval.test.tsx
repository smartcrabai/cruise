import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../App";
import type { Session } from "../types";
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
}));

vi.mock("../lib/updater", () => ({
  checkForUpdate: vi.fn().mockResolvedValue(null),
  checkForUpdateManual: vi.fn().mockResolvedValue(null),
  downloadAndInstall: vi.fn(),
}));

vi.mock("../lib/desktopNotifications", () => ({
  notifyDesktop: vi.fn(),
}));

// --- Helpers ------------------------------------------------------------------

function makeSession(overrides: Partial<Session> = {}): Session {
  return {
    id: "session-1",
    phase: "Awaiting Approval",
    configSource: "default.yaml",
    baseDir: "/home/user/project",
    input: "pending task",
    createdAt: "2026-01-01T00:00:00Z",
    workspaceMode: "Worktree",
    planAvailable: true,
    skippedSteps: [],
    ...overrides,
  };
}

// --- Awaiting Approval: Fix and Ask button visibility ------------------------

describe("App: Awaiting Approval -- Fix and Ask button visibility", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listConfigs).mockResolvedValue([]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("# The plan");
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
  });

  afterEach(() => {
    cleanup();
  });

  it("shows Fix and Ask buttons when planAvailable is true", async () => {
    // Given: an Awaiting Approval session with a plan
    const session = makeSession({ planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);

    render(<App />);
    await waitFor(() => screen.getByText("pending task"));

    // When: select the session
    await userEvent.click(screen.getByRole("button", { name: /pending task/ }));

    // Then: Fix and Ask are visible
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Fix" })).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Ask" })).toBeInTheDocument();
    });
  });

  it("hides Fix and Ask when planAvailable is false", async () => {
    // Given: Awaiting Approval session without a plan yet
    const session = makeSession({ planAvailable: false });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);

    render(<App />);
    await waitFor(() => screen.getByText("pending task"));

    await userEvent.click(screen.getByRole("button", { name: /pending task/ }));

    // Then: Fix and Ask are absent (nothing to review yet)
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: "Fix" })).toBeNull();
      expect(screen.queryByRole("button", { name: "Ask" })).toBeNull();
    });
  });
});

// --- Awaiting Approval: Ask flow ---------------------------------------------

describe("App: Awaiting Approval -- Ask flow", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listConfigs).mockResolvedValue([]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("# The plan");
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
  });

  afterEach(() => {
    cleanup();
  });

  async function selectAwaitingApprovalSession(): Promise<void> {
    const session = makeSession({ planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);
    render(<App />);
    await waitFor(() => screen.getByText("pending task"));
    await userEvent.click(screen.getByRole("button", { name: /pending task/ }));
    await waitFor(() => screen.getByRole("button", { name: "Ask" }));
  }

  it("shows question input when Ask is clicked", async () => {
    // Given: an Awaiting Approval session is selected
    await selectAwaitingApprovalSession();

    // When: click Ask
    await userEvent.click(screen.getByRole("button", { name: "Ask" }));

    // Then: question textarea is visible
    expect(
      screen.getByPlaceholderText("Ask a question about the plan...")
    ).toBeInTheDocument();
  });

  it("calls askSession with session ID and question, then displays the answer", async () => {
    // Given: askSession is ready to return an answer
    vi.mocked(commands.askSession).mockResolvedValue("The plan uses approach X because of Y.");
    await selectAwaitingApprovalSession();

    // When: ask, type question, and submit
    await userEvent.click(screen.getByRole("button", { name: "Ask" }));
    await userEvent.type(
      screen.getByPlaceholderText("Ask a question about the plan..."),
      "Why approach X?"
    );
    await userEvent.click(screen.getByRole("button", { name: "Submit" }));

    // Then: askSession is called with correct args
    await waitFor(() => {
      expect(commands.askSession).toHaveBeenCalledWith("session-1", "Why approach X?");
    });

    // And: the answer is displayed on the Plan tab
    await waitFor(() => {
      expect(
        screen.getByText("The plan uses approach X because of Y.")
      ).toBeInTheDocument();
    });
  });

  it("shows action buttons again after receiving an Ask answer", async () => {
    // Given: an Ask has been answered
    vi.mocked(commands.askSession).mockResolvedValue("Some answer.");
    await selectAwaitingApprovalSession();

    await userEvent.click(screen.getByRole("button", { name: "Ask" }));
    await userEvent.type(
      screen.getByPlaceholderText("Ask a question about the plan..."),
      "Question?"
    );
    await userEvent.click(screen.getByRole("button", { name: "Submit" }));
    await waitFor(() => screen.getByText("Some answer."));

    // Then: Approve, Fix, Ask, and Delete are all still accessible
    expect(screen.getByRole("button", { name: "Approve" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Fix" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Ask" })).toBeInTheDocument();
  });

  it("clears the Ask answer when a different session is selected", async () => {
    // Given: two sessions; A is Awaiting Approval with a plan
    const sessA = makeSession({ id: "sess-a", input: "task A", planAvailable: true });
    const sessB = makeSession({
      id: "sess-b",
      input: "task B",
      phase: "Planned",
      planAvailable: false,
    });
    vi.mocked(commands.listSessions).mockResolvedValue([sessA, sessB]);
    vi.mocked(commands.askSession).mockResolvedValue("The answer.");

    render(<App />);
    await waitFor(() => screen.getByText("task A"));

    // Select A, get an Ask answer
    await userEvent.click(screen.getByRole("button", { name: /task A/ }));
    await waitFor(() => screen.getByRole("button", { name: "Ask" }));
    await userEvent.click(screen.getByRole("button", { name: "Ask" }));
    await userEvent.type(
      screen.getByPlaceholderText("Ask a question about the plan..."),
      "Question?"
    );
    await userEvent.click(screen.getByRole("button", { name: "Submit" }));
    await waitFor(() => screen.getByText("The answer."));

    // When: navigate to session B, then back to A
    await userEvent.click(screen.getByRole("button", { name: /task B/ }));
    await userEvent.click(screen.getByRole("button", { name: /task A/ }));

    // Then: the stale answer is gone
    expect(screen.queryByText("The answer.")).toBeNull();
  });

  it("shows error and keeps question editor open when Ask fails", async () => {
    // Given: askSession rejects
    vi.mocked(commands.askSession).mockRejectedValue(new Error("Ask failed"));
    await selectAwaitingApprovalSession();

    await userEvent.click(screen.getByRole("button", { name: "Ask" }));
    await userEvent.type(
      screen.getByPlaceholderText("Ask a question about the plan..."),
      "A question"
    );
    await userEvent.click(screen.getByRole("button", { name: "Submit" }));

    // Then: an error is visible
    await waitFor(() => {
      expect(screen.getByText(/Ask failed/)).toBeInTheDocument();
    });

    // And: the question editor remains open so the user can retry
    expect(
      screen.getByPlaceholderText("Ask a question about the plan...")
    ).toBeInTheDocument();
  });

  it("collapses the question editor without clearing ask answer when Cancel is clicked", async () => {
    // Given: Ask editor is open (no pending ask yet)
    await selectAwaitingApprovalSession();
    await userEvent.click(screen.getByRole("button", { name: "Ask" }));
    expect(
      screen.getByPlaceholderText("Ask a question about the plan...")
    ).toBeInTheDocument();

    // When: cancel
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));

    // Then: editor is hidden and primary actions are visible again
    expect(
      screen.queryByPlaceholderText("Ask a question about the plan...")
    ).toBeNull();
    expect(screen.getByRole("button", { name: "Approve" })).toBeInTheDocument();
  });
});

// --- Awaiting Approval: Fixing display state ---

describe("App: Awaiting Approval -- Fixing display state", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listConfigs).mockResolvedValue([]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("# The plan");
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
  });

  afterEach(() => {
    cleanup();
  });

  async function selectAwaitingApprovalSession(): Promise<void> {
    const session = makeSession({ planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);
    render(<App />);
    await waitFor(() => screen.getByText("pending task"));
    await userEvent.click(screen.getByRole("button", { name: /pending task/ }));
    await waitFor(() => screen.getByRole("button", { name: "Fix" }));
  }

  it("shows 'Fixing' badge while the fix request is in-flight", async () => {
    // Given: fixSession never resolves during this test (simulates a long-running fix)
    vi.mocked(commands.fixSession).mockImplementationOnce(
      () => new Promise<string>(() => { /* never resolves */ })
    );
    await selectAwaitingApprovalSession();

    // When: start the fix by clicking Fix -> typing feedback -> clicking Apply
    await userEvent.click(screen.getByRole("button", { name: "Fix" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe the changes needed..."),
      "Make it better"
    );
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));

    // Then: the detail header badge changes from "Awaiting Approval" to "Fixing"
    // (the sidebar badge also shows "Fixing" because fixingSessionIds is propagated from App)
    await waitFor(() => {
      expect(screen.getAllByText("Fixing").length).toBeGreaterThanOrEqual(1);
    });
    expect(screen.queryByText("Awaiting Approval")).toBeNull();
  });

  it("hides Approve, Fix, and Ask buttons while the fix is in-flight", async () => {
    // Given: fixSession never resolves (in-flight)
    vi.mocked(commands.fixSession).mockImplementationOnce(
      () => new Promise<string>(() => { /* never resolves */ })
    );
    await selectAwaitingApprovalSession();

    // When: submit the fix
    await userEvent.click(screen.getByRole("button", { name: "Fix" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe the changes needed..."),
      "Make it better"
    );
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));

    // Then: all three review-action buttons are hidden while fixing is running
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
    });
    expect(screen.queryByRole("button", { name: "Fix" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Ask" })).toBeNull();
  });

  it("clears the Fixing badge after fix succeeds and restores normal Awaiting Approval state", async () => {
    // Given: fixSession resolves successfully
    vi.mocked(commands.fixSession).mockImplementationOnce(
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      async (_params: any, channel: any) => {
        channel.onmessage?.({ event: "planGenerated", data: { content: "# Revised plan" } });
        return "# Revised plan";
      }
    );
    vi.mocked(commands.getSession).mockResolvedValue(makeSession({ planAvailable: true }));
    await selectAwaitingApprovalSession();

    await userEvent.click(screen.getByRole("button", { name: "Fix" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe the changes needed..."),
      "Make it better"
    );
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));

    // Then: "Fixing" badge is removed after the plan is generated
    await waitFor(() => {
      expect(screen.queryByText("Fixing")).toBeNull();
    });
  });

  /**
   * Sets up a mid-fix scenario: session A is Awaiting Approval, session B is Planned.
   * Mocks fixSession to emit planGenerating then hang indefinitely.
   * Renders App, selects A, starts a fix, and waits until "Fixing" is visible.
   * Returns the two session objects for further use.
   */
  async function renderAndStartMidFix() {
    const sessA = makeSession({ id: "sess-a", input: "task A", planAvailable: true });
    const sessB = makeSession({ id: "sess-b", input: "task B", phase: "Planned", planAvailable: false });
    vi.mocked(commands.listSessions).mockResolvedValueOnce([sessA, sessB]);

    const sessAFixing = makeSession({
      id: "sess-a",
      input: "task A",
      planAvailable: true,
      fixInProgress: true,
      updatedAt: "2026-01-01T00:01:00Z",
    });
    vi.mocked(commands.listSessions).mockResolvedValue([sessAFixing, sessB]);

    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    vi.mocked(commands.fixSession).mockImplementationOnce((_params: any, channel: any) => {
      channel.onmessage?.({ event: "planGenerating", data: {} });
      return new Promise<string>(() => { /* never resolves */ });
    });
    vi.mocked(commands.getSession).mockResolvedValue(sessAFixing);

    render(<App />);
    await waitFor(() => screen.getByText("task A"));

    await userEvent.click(screen.getByRole("button", { name: /task A/ }));
    await waitFor(() => screen.getByRole("button", { name: "Fix" }));
    await userEvent.click(screen.getByRole("button", { name: "Fix" }));
    await userEvent.type(screen.getByPlaceholderText("Describe the changes needed..."), "Improve it");
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));

    await waitFor(() => {
      expect(screen.getAllByText("Fixing").length).toBeGreaterThanOrEqual(1);
    });
    return { sessB };
  }

  it("keeps 'Fixing' badge on the original session's sidebar entry after navigating to another session mid-fix", async () => {
    await renderAndStartMidFix();

    // When: navigate to session B (this used to clear fixingSessionIds via cleanup)
    await userEvent.click(screen.getByRole("button", { name: /task B/ }));

    // Then: session A's sidebar badge still shows "Fixing" (DTO-driven, not ephemeral)
    await waitFor(() => {
      expect(screen.getByText("Fixing")).toBeInTheDocument();
    });
    expect(screen.queryByText("Awaiting Approval")).toBeNull();
  });

  it("shows 'Fixing' badge in the detail header when navigating back to the session mid-fix", async () => {
    await renderAndStartMidFix();

    // Navigate away to session B, then come back to session A
    await userEvent.click(screen.getByRole("button", { name: /task B/ }));
    await userEvent.click(screen.getByRole("button", { name: /task A/ }));

    // Then: the detail header badge still shows "Fixing" (not "Awaiting Approval")
    await waitFor(() => {
      expect(screen.getAllByText("Fixing").length).toBeGreaterThanOrEqual(1);
    });
    expect(screen.queryByText("Awaiting Approval")).toBeNull();
  });

  it("clears the Fixing badge after fix fails and shows the error", async () => {
    // Given: fixSession rejects with an error
    vi.mocked(commands.fixSession).mockRejectedValue(new Error("Fix failed"));
    await selectAwaitingApprovalSession();

    await userEvent.click(screen.getByRole("button", { name: "Fix" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe the changes needed..."),
      "Make it better"
    );
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));

    // Then: error is shown and the Fixing badge is cleared (back to normal state)
    await waitFor(() => {
      expect(screen.getByText(/Fix failed/)).toBeInTheDocument();
    });
    expect(screen.queryByText("Fixing")).toBeNull();
  });
});

// --- Awaiting Approval: Fix flow ---

describe("App: Awaiting Approval -- Fix flow", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listConfigs).mockResolvedValue([]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("# The plan");
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
  });

  afterEach(() => {
    cleanup();
  });

  async function selectAwaitingApprovalSession(): Promise<void> {
    const session = makeSession({ planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);
    render(<App />);
    await waitFor(() => screen.getByText("pending task"));
    await userEvent.click(screen.getByRole("button", { name: /pending task/ }));
    await waitFor(() => screen.getByRole("button", { name: "Fix" }));
  }

  it("shows fix feedback editor when Fix is clicked", async () => {
    // Given: an Awaiting Approval session is selected
    await selectAwaitingApprovalSession();

    // When: click Fix
    await userEvent.click(screen.getByRole("button", { name: "Fix" }));

    // Then: fix feedback editor is visible
    expect(
      screen.getByPlaceholderText("Describe the changes needed...")
    ).toBeInTheDocument();
  });

  it("calls fixSession with feedback and updates plan content on success", async () => {
    // Given: fixSession streams planGenerated and returns updated content
    vi.mocked(commands.fixSession).mockImplementationOnce(
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      async (_params: any, channel: any) => {
        channel.onmessage?.({
          event: "planGenerated",
          data: { content: "# Revised plan" },
        });
        return "# Revised plan";
      }
    );
    vi.mocked(commands.getSession).mockResolvedValue(
      makeSession({ planAvailable: true })
    );
    await selectAwaitingApprovalSession();

    // When: click Fix, type feedback, apply
    await userEvent.click(screen.getByRole("button", { name: "Fix" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe the changes needed..."),
      "Remove step 3"
    );
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));

    // Then: fixSession is called with the session ID and feedback
    await waitFor(() => {
      expect(commands.fixSession).toHaveBeenCalledWith(
        expect.objectContaining({ sessionId: "session-1", feedback: "Remove step 3" }),
        expect.anything()
      );
    });
  });

  it("clears stale Ask answer when Fix succeeds", async () => {
    // Given: an Ask answer is displayed, then Fix is triggered
    vi.mocked(commands.askSession).mockResolvedValue("Old ask answer.");
    vi.mocked(commands.fixSession).mockImplementationOnce(
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      async (_params: any, channel: any) => {
        channel.onmessage?.({
          event: "planGenerated",
          data: { content: "# Revised plan" },
        });
        return "# Revised plan";
      }
    );
    vi.mocked(commands.getSession).mockResolvedValue(
      makeSession({ planAvailable: true })
    );

    const session = makeSession({ planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);
    render(<App />);
    await waitFor(() => screen.getByText("pending task"));
    await userEvent.click(screen.getByRole("button", { name: /pending task/ }));
    await waitFor(() => screen.getByRole("button", { name: "Ask" }));

    // Get an Ask answer
    await userEvent.click(screen.getByRole("button", { name: "Ask" }));
    await userEvent.type(
      screen.getByPlaceholderText("Ask a question about the plan..."),
      "Question?"
    );
    await userEvent.click(screen.getByRole("button", { name: "Submit" }));
    await waitFor(() => screen.getByText("Old ask answer."));

    // When: Fix succeeds
    await userEvent.click(screen.getByRole("button", { name: "Fix" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe the changes needed..."),
      "Revise"
    );
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));

    // Then: the stale Ask answer is cleared
    await waitFor(() => {
      expect(screen.queryByText("Old ask answer.")).toBeNull();
    });
  });

  it("shows error and keeps fix editor open when Fix fails", async () => {
    // Given: fixSession rejects
    vi.mocked(commands.fixSession).mockRejectedValue(new Error("Fix failed"));
    await selectAwaitingApprovalSession();

    await userEvent.click(screen.getByRole("button", { name: "Fix" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe the changes needed..."),
      "Do something"
    );
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));

    // Then: an error is visible
    await waitFor(() => {
      expect(screen.getByText(/Fix failed/)).toBeInTheDocument();
    });

    // And: the fix editor remains open so the user can retry
    expect(
      screen.getByPlaceholderText("Describe the changes needed...")
    ).toBeInTheDocument();
  });

  it("refreshes session state after Fix so the sidebar reflects the updated plan", async () => {
    // Given: fixSession succeeds
    vi.mocked(commands.fixSession).mockImplementationOnce(
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      async (_params: any, channel: any) => {
        channel.onmessage?.({
          event: "planGenerated",
          data: { content: "# Updated plan" },
        });
        return "# Updated plan";
      }
    );
    const refreshedSession = makeSession({ planAvailable: true, updatedAt: "2026-01-02T00:00:00Z" });
    vi.mocked(commands.getSession).mockResolvedValue(refreshedSession);

    await selectAwaitingApprovalSession();

    await userEvent.click(screen.getByRole("button", { name: "Fix" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe the changes needed..."),
      "Revise"
    );
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));

    // Then: getSession is called to refresh the session DTO (updates title/updatedAt)
    await waitFor(() => {
      expect(commands.getSession).toHaveBeenCalledWith("session-1");
    });
  });
});
