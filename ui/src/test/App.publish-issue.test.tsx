import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../App";
import type { Session } from "../types";
import * as commands from "../lib/commands";
import * as opener from "@tauri-apps/plugin-opener";

// --- Module mocks ------------------------------------------------------------------

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
  publishPlanIssue: vi.fn(),
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
  getNewSessionDraft: vi.fn().mockResolvedValue(null),
  saveNewSessionDraft: vi.fn().mockResolvedValue(undefined),
  clearNewSessionDraft: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("../lib/updater", () => ({
  checkForUpdate: vi.fn().mockResolvedValue(null),
  checkForUpdateManual: vi.fn().mockResolvedValue(null),
  downloadAndInstall: vi.fn(),
}));

vi.mock("../lib/desktopNotifications", () => ({
  notifyDesktop: vi.fn(),
}));

// --- Helpers -----------------------------------------------------------------------

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

function setupBaseMocks() {
  vi.clearAllMocks();
  vi.mocked(commands.listConfigs).mockResolvedValue([]);
  vi.mocked(commands.getSessionLog).mockResolvedValue("");
  vi.mocked(commands.getSessionPlan).mockResolvedValue("# The plan");
  vi.mocked(commands.listDirectory).mockResolvedValue([]);
  vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
  vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
  // handleDeleteConfirmed (shared by Delete and the post-publish cleanup) always
  // calls deleteSession -- the session was already removed server-side by
  // publish_plan_issue_and_delete, so this call is a no-op cleanup here.
  vi.mocked(commands.deleteSession).mockResolvedValue(undefined);
}

async function navigateToSession(session: Session): Promise<void> {
  vi.mocked(commands.listSessions).mockResolvedValue([session]);
  render(<App />);
  await waitFor(() => screen.getByText(session.input));
  await userEvent.click(screen.getByRole("button", { name: new RegExp(session.input) }));
  await waitFor(() => screen.getByRole("button", { name: "Publish as Issue" }));
}

// --- Tests ---------------------------------------------------------------------------

describe("App: Publish as Issue button visibility", () => {
  beforeEach(setupBaseMocks);
  afterEach(() => {
    cleanup();
  });

  it("shows Publish as Issue for an Awaiting Approval session with a plan", async () => {
    // Given: an Awaiting Approval session with a plan ready for review (existing behavior)
    const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);

    render(<App />);
    await waitFor(() => screen.getByText("test task"));
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    // Then
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Publish as Issue" })).toBeInTheDocument();
    });
  });

  it("shows Publish as Issue for a Planned session with a plan", async () => {
    // Given: a Planned session (already approved) -- can be redirected to a
    // GitHub issue + `@cruise run` instead of running locally
    const session = makeSession({ phase: "Planned", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);

    render(<App />);
    await waitFor(() => screen.getByText("test task"));
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    // Then
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Publish as Issue" })).toBeInTheDocument();
    });
  });

  it("hides Publish as Issue for a Planned session without a plan", async () => {
    // Given: a Planned session whose plan.md is missing/empty
    const session = makeSession({ phase: "Planned", planAvailable: false });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);

    render(<App />);
    await waitFor(() => screen.getByText("test task"));
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    // Then
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: "Publish as Issue" })).toBeNull();
    });
  });
});

describe("App: Publish as Issue dialog defaults", () => {
  beforeEach(setupBaseMocks);
  afterEach(() => {
    cleanup();
  });

  it("opens with the checkbox unchecked by default for an Awaiting Approval session", async () => {
    // Given
    const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });
    await navigateToSession(session);

    // When
    await userEvent.click(screen.getByRole("button", { name: "Publish as Issue" }));

    // Then: the review-issue use case defaults to not triggering the Action
    await waitFor(() => {
      expect(screen.getByRole("checkbox")).not.toBeChecked();
    });
  });

  it("opens with the checkbox checked by default for a Planned session", async () => {
    // Given: Planned publishing replaces running locally, so the Action should
    // be triggered by default
    const session = makeSession({ phase: "Planned", planAvailable: true });
    await navigateToSession(session);

    // When
    await userEvent.click(screen.getByRole("button", { name: "Publish as Issue" }));

    // Then
    await waitFor(() => {
      expect(screen.getByRole("checkbox")).toBeChecked();
    });
  });
});

describe("App: Publish as Issue confirm flow", () => {
  beforeEach(setupBaseMocks);
  afterEach(() => {
    cleanup();
  });

  it("calls publishPlanIssue with triggerCruise=false by default for an Awaiting Approval session", async () => {
    // Given
    vi.mocked(commands.publishPlanIssue).mockResolvedValue({
      url: "https://github.com/owner/repo/issues/42",
      repo: "owner/repo",
    });
    const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });
    await navigateToSession(session);

    // When: open the dialog and confirm without touching the checkbox
    await userEvent.click(screen.getByRole("button", { name: "Publish as Issue" }));
    await userEvent.click(await screen.findByRole("button", { name: "Publish" }));

    // Then
    await waitFor(() => {
      expect(commands.publishPlanIssue).toHaveBeenCalledWith("session-1", false);
    });
  });

  it("calls publishPlanIssue with triggerCruise=true by default for a Planned session", async () => {
    // Given
    vi.mocked(commands.publishPlanIssue).mockResolvedValue({
      url: "https://github.com/owner/repo/issues/43",
      repo: "owner/repo",
    });
    const session = makeSession({ phase: "Planned", planAvailable: true });
    await navigateToSession(session);

    // When: open the dialog and confirm without touching the checkbox
    await userEvent.click(screen.getByRole("button", { name: "Publish as Issue" }));
    await userEvent.click(await screen.findByRole("button", { name: "Publish" }));

    // Then
    await waitFor(() => {
      expect(commands.publishPlanIssue).toHaveBeenCalledWith("session-1", true);
    });
  });

  it("removes the session from the sidebar and opens the issue URL after a successful publish", async () => {
    // Given
    vi.mocked(commands.publishPlanIssue).mockResolvedValue({
      url: "https://github.com/owner/repo/issues/44",
      repo: "owner/repo",
    });
    const session = makeSession({ phase: "Planned", planAvailable: true });
    await navigateToSession(session);

    // When
    await userEvent.click(screen.getByRole("button", { name: "Publish as Issue" }));
    await userEvent.click(await screen.findByRole("button", { name: "Publish" }));

    // Then: the session disappears from the sidebar
    await waitFor(() => {
      expect(screen.queryAllByText("test task")).toHaveLength(0);
    });

    // And: the created issue is opened in the browser
    expect(opener.openUrl).toHaveBeenCalledWith("https://github.com/owner/repo/issues/44");
  });
});
