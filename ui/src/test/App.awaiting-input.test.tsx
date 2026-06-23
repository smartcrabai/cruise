import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup, act, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../App";
import type { Session } from "../types";
import * as commands from "../lib/commands";
import * as desktopNotifications from "../lib/desktopNotifications";

// --- Module mocks ---------------------------------------------------------------

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
  listGithubRepos: vi.fn(),
  createSession: vi.fn(),
  approveSession: vi.fn(),
  discardSession: vi.fn(),
  getSession: vi.fn(),
  getSessionLog: vi.fn(),
  getSessionPlan: vi.fn(),
  getNewSessionHistorySummary: vi.fn().mockResolvedValue({ recentWorkingDirs: [] }),
  getNewSessionConfigDefaults: vi.fn().mockResolvedValue({
    steps: [],
    afterPrSteps: [],
    defaultSkippedSteps: [],
  }),
  getNewSessionDraft: vi.fn().mockResolvedValue(null),
  saveNewSessionDraft: vi.fn().mockResolvedValue(undefined),
  clearNewSessionDraft: vi.fn().mockResolvedValue(undefined),
  listDirectory: vi.fn(),
  getUpdateReadiness: vi.fn(),
  cleanSessions: vi.fn(),
  deleteSession: vi.fn(),
  runSession: vi.fn(),
  cancelSession: vi.fn(),
  resetSession: vi.fn(),
  respondToAsk: vi.fn(),
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

// --- Helpers --------------------------------------------------------------------

function makeSession(overrides: Partial<Session> = {}): Session {
  return {
    id: "session-1",
    phase: "Awaiting Input",
    configSource: "default.yaml",
    baseDir: "/home/user/project",
    input: "planning task",
    createdAt: "2026-01-01T00:00:00Z",
    workspaceMode: "Worktree",
    skippedSteps: [],
    ...overrides,
  };
}

function setupCommonMocks() {
  vi.clearAllMocks();
  vi.mocked(commands.listSessions).mockResolvedValue([]);
  vi.mocked(commands.listConfigs).mockResolvedValue([]);
  vi.mocked(commands.getSessionLog).mockResolvedValue("");
  vi.mocked(commands.getSessionPlan).mockResolvedValue("");
  vi.mocked(commands.listDirectory).mockResolvedValue([]);
  vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
  vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
}

/**
 * Captures the live channel from createSession so we can fire channel events
 * at precise moments to trigger sidebar polls.
 */
function setupTwoPhaseCreateSession(sessionId: string) {
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
    emitSessionCreated(): void {
      capturedChannel!.onmessage?.({ event: "sessionCreated", data: { sessionId } });
    },
    emitPlanGenerated(content = "# Plan"): void {
      capturedChannel!.onmessage?.({ event: "planGenerated", data: { sessionId, content } });
      resolveCreate(sessionId);
    },
    emitPlanFailed(error = "failed"): void {
      capturedChannel!.onmessage?.({ event: "planFailed", data: { sessionId, error } });
      resolveCreate(sessionId);
    },
  };
}

// --- Input-required notification transitions ------------------------------------

describe("App: input-required notification transitions", () => {
  beforeEach(setupCommonMocks);

  afterEach(() => {
    cleanup();
  });

  it("emits input-required toast when session first gets pendingAskQuestion", async () => {
    // Given: session exists without a pendingAskQuestion initially
    const control = setupTwoPhaseCreateSession("sess-ask");
    vi.mocked(commands.getSession).mockResolvedValue(
      makeSession({ id: "sess-ask", pendingAskQuestion: "Which auth strategy?" })
    );

    render(<App />);
    await waitFor(() => screen.getByRole("button", { name: "+ New" }));

    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "planning task"
    );
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    // sessionCreated: session is AwaitingInput but no pendingAskQuestion yet
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-ask", phase: "Awaiting Input", pendingAskQuestion: undefined }),
    ]);
    await act(async () => { control.emitSessionCreated(); });
    await waitFor(() => {
      expect(
        screen.getByPlaceholderText("Describe what you want to implement...")
      ).toHaveValue("");
    });

    // When: planGenerated fires -> sidebar poll returns the question
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-ask", phase: "Awaiting Input", pendingAskQuestion: "Which auth strategy?" }),
    ]);
    await act(async () => { control.emitPlanGenerated(); });

    // Then: input-required toast appears
    await waitFor(() => expect(screen.getByText("Action required")).toBeInTheDocument());
  });

  it("emits input-required notification again when question text changes (second ask_user call)", async () => {
    // Given: session already has pendingAskQuestion Q1 after first poll
    const control = setupTwoPhaseCreateSession("sess-ask2");
    vi.mocked(commands.getSession).mockResolvedValue(
      makeSession({ id: "sess-ask2", pendingAskQuestion: "Q2?" })
    );

    render(<App />);
    await waitFor(() => screen.getByRole("button", { name: "+ New" }));

    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "planning task"
    );
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    // sessionCreated: session has Q1
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-ask2", phase: "Awaiting Input", pendingAskQuestion: "Q1?" }),
    ]);
    await act(async () => { control.emitSessionCreated(); });
    await waitFor(() => {
      expect(
        screen.getByPlaceholderText("Describe what you want to implement...")
      ).toHaveValue("");
    });

    // When: planGenerated fires -> sidebar poll returns Q2 (question changed)
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-ask2", phase: "Awaiting Input", pendingAskQuestion: "Q2?" }),
    ]);
    await act(async () => { control.emitPlanGenerated(); });

    // Then: input-required toast appears (Q1→Q2 transition detected)
    await waitFor(() => expect(screen.getByText("Action required")).toBeInTheDocument());
  });

  it("does not emit input-required notification on app startup when session already has pendingAskQuestion", async () => {
    // Given: app starts with a session that already has a pendingAskQuestion
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({
        id: "existing-ask",
        phase: "Awaiting Input",
        pendingAskQuestion: "Pre-existing question?",
      }),
    ]);

    render(<App />);
    // Wait for initial load
    await waitFor(() => screen.getByText("planning task"));
    await act(async () => { await new Promise<void>((r) => setTimeout(r, 20)); });

    // Then: no input-required toast (startup suppression: first snapshot is never notified)
    expect(screen.queryByText("Action required")).not.toBeInTheDocument();
    expect(vi.mocked(desktopNotifications.notifyDesktop)).not.toHaveBeenCalledWith(
      expect.anything(),
      expect.stringContaining("Action required"),
    );
  });

  it("does not emit input-required notification when pendingAskQuestion is unchanged", async () => {
    // Given: session has pendingAskQuestion Q1 in both polls (no transition)
    const control = setupTwoPhaseCreateSession("sess-nodiff");
    vi.mocked(commands.getSession).mockResolvedValue(
      makeSession({ id: "sess-nodiff", pendingAskQuestion: "Same question?" })
    );

    render(<App />);
    await waitFor(() => screen.getByRole("button", { name: "+ New" }));

    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "planning task"
    );
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    // sessionCreated: session has Q1
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-nodiff", phase: "Awaiting Input", pendingAskQuestion: "Same question?" }),
    ]);
    await act(async () => { control.emitSessionCreated(); });
    await waitFor(() => {
      expect(
        screen.getByPlaceholderText("Describe what you want to implement...")
      ).toHaveValue("");
    });

    // When: planGenerated fires -> sidebar poll returns the same Q1 (no change)
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-nodiff", phase: "Awaiting Input", pendingAskQuestion: "Same question?" }),
    ]);
    await act(async () => { control.emitPlanGenerated(); });

    // Then: no toast (same pendingAskQuestion → not a new transition)
    await act(async () => { await new Promise<void>((r) => setTimeout(r, 20)); });
    expect(screen.queryByText("Action required")).not.toBeInTheDocument();
  });
});

// --- Awaiting Input sidebar badge -----------------------------------------------

describe("App: Awaiting Input sidebar blue dot badge", () => {
  beforeEach(setupCommonMocks);

  afterEach(() => {
    cleanup();
  });

  it("shows blue dot in sidebar for a session in Awaiting Input phase", async () => {
    // Given: a session in Awaiting Input phase
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-badge", phase: "Awaiting Input", pendingAskQuestion: "Choose DB?" }),
    ]);

    render(<App />);

    // Then: the input-required blue dot is visible in the sidebar entry
    await waitFor(() =>
      expect(screen.getByLabelText("user input required")).toBeInTheDocument()
    );
  });

  it("does not show approval-ready dot for an Awaiting Input session", async () => {
    // Given: a session in Awaiting Input phase with planAvailable
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({
        id: "sess-badge2",
        phase: "Awaiting Input",
        planAvailable: true,
        pendingAskQuestion: "Choose DB?",
      }),
    ]);

    render(<App />);
    await waitFor(() => screen.getByText("planning task"));

    // Then: the approval-ready dot must not appear for Awaiting Input
    expect(screen.queryByLabelText("plan ready for approval")).not.toBeInTheDocument();
  });
});
