import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup, act, fireEvent } from "@testing-library/react";
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
  listGithubRepos: vi.fn(),
  createSession: vi.fn(),
  approveSession: vi.fn(),
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

function setupNewSessionMocks() {
  vi.clearAllMocks();
  vi.mocked(commands.listSessions).mockResolvedValue([]);
  vi.mocked(commands.listConfigs).mockResolvedValue([]);
  vi.mocked(commands.listGithubRepos).mockResolvedValue([]);
  vi.mocked(commands.getSessionLog).mockResolvedValue("");
  vi.mocked(commands.getSessionPlan).mockResolvedValue("");
  vi.mocked(commands.getNewSessionHistorySummary).mockResolvedValue({ recentWorkingDirs: [] });
  vi.mocked(commands.getNewSessionConfigDefaults).mockResolvedValue({
    steps: [],
    afterPrSteps: [],
    defaultSkippedSteps: [],
  });
  vi.mocked(commands.getNewSessionDraft).mockResolvedValue(null);
  vi.mocked(commands.saveNewSessionDraft).mockResolvedValue(undefined);
  vi.mocked(commands.clearNewSessionDraft).mockResolvedValue(undefined);
  vi.mocked(commands.listDirectory).mockResolvedValue([]);
  vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
  vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
}

function makeSession(overrides: Partial<Session> = {}): Session {
  return {
    id: "session-1",
    phase: "Planned",
    configSource: "default.yaml",
    baseDir: "/home/user/project",
    input: "test task",
    createdAt: "2026-01-01T00:00:00Z",
    workspaceMode: "Worktree",
    skippedSteps: [],
    ...overrides,
  };
}

const BUILD_ONLY_STEP = [
  {
    id: "build",
    expandedStepIds: ["build"],
    children: [],
  },
];

const BUILD_AND_REVIEW_STEPS = [
  {
    id: "build",
    expandedStepIds: ["build"],
    children: [],
  },
  {
    id: "review-pass",
    expandedStepIds: ["review-pass/simplify"],
    children: [
      {
        id: "review-pass/simplify",
        expandedStepIds: ["review-pass/simplify"],
        children: [],
      },
    ],
  },
];

// --- New Session draft state persistence -------------------------------------

describe("App: New Session draft state persistence", () => {
  beforeEach(setupNewSessionMocks);

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

  it("does not overwrite user-typed Working Directory with default loaded from history summary on remount", async () => {
    // Given: persisted history has a specific last working directory
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-1", input: "existing task" }),
    ]);
    vi.mocked(commands.getNewSessionHistorySummary).mockResolvedValue({
      lastWorkingDir: "/from/history",
      recentWorkingDirs: ["/from/history"],
    });
    render(<App />);
    await waitFor(() => screen.getByText("existing task"));

    // Navigate to New Session, type a working directory, then navigate away
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    const baseDirInput = screen.getByPlaceholderText("e.g. /Users/you/projects/myapp");
    await userEvent.clear(baseDirInput);
    await userEvent.type(baseDirInput, "/my/typed/dir");
    await userEvent.click(screen.getByRole("button", { name: /existing task/ }));

    // When: navigate back to New Session (triggers remount, history summary loads again)
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await act(async () => { await new Promise<void>((r) => setTimeout(r, 50)); });

    // Then: the user-typed value is NOT overwritten by the persisted default
    expect(
      screen.getByPlaceholderText("e.g. /Users/you/projects/myapp")
    ).toHaveValue("/my/typed/dir");
  });

  it("applies persisted config and working directory defaults when the draft is empty", async () => {
    vi.mocked(commands.listConfigs).mockResolvedValue([
      { name: "team.yaml", path: "/Users/takumi/.cruise/team.yaml" },
    ]);
    vi.mocked(commands.getNewSessionHistorySummary).mockResolvedValue({
      lastRequestedConfigPath: "/Users/takumi/.cruise/team.yaml",
      lastWorkingDir: "/repos/cruise",
      recentWorkingDirs: ["/repos/cruise", "/repos/other"],
    });

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    await waitFor(() => {
      expect(screen.getByLabelText("Config")).toHaveValue("/Users/takumi/.cruise/team.yaml");
    });
    expect(
      screen.getByPlaceholderText("e.g. /Users/you/projects/myapp")
    ).toHaveValue("/repos/cruise");
    expect(screen.getByRole("button", { name: "/repos/other" })).toBeInTheDocument();
  });

  it("shows recent working directories and lets the user reselect one", async () => {
    vi.mocked(commands.getNewSessionHistorySummary).mockResolvedValue({
      recentWorkingDirs: ["/repos/a", "/repos/b"],
    });

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    await userEvent.click(screen.getByRole("button", { name: "/repos/b" }));
    expect(
      screen.getByPlaceholderText("e.g. /Users/you/projects/myapp")
    ).toHaveValue("/repos/b");
  });

  it("loads skip-step defaults from the resolved config, including auto mode", async () => {
    vi.mocked(commands.getNewSessionConfigDefaults).mockResolvedValue({
      steps: [
        { id: "write-tests", expandedStepIds: ["write-tests"], children: [] },
        { id: "implement", expandedStepIds: ["implement"], children: [] },
      ],
      afterPrSteps: [],
      defaultSkippedSteps: ["write-tests"],
    });

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    const writeTests = await screen.findByRole("checkbox", { name: "write-tests" });
    const implement = screen.getByRole("checkbox", { name: "implement" });
    expect(writeTests).toBeChecked();
    expect(implement).not.toBeChecked();
  });

});

describe("App: New Session draft persistence via IPC", () => {
  beforeEach(setupNewSessionMocks);

  afterEach(() => {
    cleanup();
  });

  it("restores draft input, configPath, and baseDir from persisted draft on first mount", async () => {
    vi.mocked(commands.getNewSessionDraft).mockResolvedValue({
      input: "my stored task",
      configPath: "/tmp/config.yaml",
      baseDir: "/tmp/project",
      skippedSteps: [],
    });
    vi.mocked(commands.listConfigs).mockResolvedValue([
      { path: "/tmp/config.yaml", name: "config.yaml" },
    ]);

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    await waitFor(() => {
      expect(
        screen.getByPlaceholderText("Describe what you want to implement...")
      ).toHaveValue("my stored task");
    });
    expect(screen.getByLabelText("Config")).toHaveValue("/tmp/config.yaml");
    expect(
      screen.getByPlaceholderText("e.g. /Users/you/projects/myapp")
    ).toHaveValue("/tmp/project");
  });

  it("does not overwrite already-filled draft with persisted values on navigation back", async () => {
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-1", input: "existing task" }),
    ]);
    vi.mocked(commands.getNewSessionDraft).mockResolvedValue({
      input: "stored task",
      configPath: "/tmp/config.yaml",
      baseDir: "/tmp/stored",
      skippedSteps: [],
    });
    vi.mocked(commands.listConfigs).mockResolvedValue([
      { path: "/tmp/config.yaml", name: "config.yaml" },
    ]);

    render(<App />);
    await waitFor(() => screen.getByText("existing task"));

    // Navigate to New Session -- draft is loaded from persistence
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await waitFor(() => {
      expect(
        screen.getByPlaceholderText("Describe what you want to implement...")
      ).toHaveValue("stored task");
    });

    // User types a new task and working dir
    await userEvent.clear(screen.getByPlaceholderText("Describe what you want to implement..."));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "my typed task"
    );
    const baseDirInput = screen.getByPlaceholderText("e.g. /Users/you/projects/myapp");
    await userEvent.clear(baseDirInput);
    await userEvent.type(baseDirInput, "/my/typed/dir");

    // Navigate away and back
    await userEvent.click(screen.getByRole("button", { name: /existing task/ }));
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // Then: user-typed values preserved, persisted draft is NOT reapplied
    expect(
      screen.getByPlaceholderText("Describe what you want to implement...")
    ).toHaveValue("my typed task");
    expect(baseDirInput).toHaveValue("/my/typed/dir");
  });
});

describe("App: New Session draft save on value changes", () => {
  beforeEach(() => {
    setupNewSessionMocks();
    vi.useFakeTimers();
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("calls saveNewSessionDraft with debounce after task input changes", async () => {
    vi.mocked(commands.getNewSessionHistorySummary).mockResolvedValue({
      recentWorkingDirs: [],
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "+ New" }));

    const taskTextarea = screen.getByPlaceholderText("Describe what you want to implement...");
    fireEvent.change(taskTextarea, { target: { value: "debounced save" } });

    // Not yet called because of 500ms debounce
    expect(commands.saveNewSessionDraft).not.toHaveBeenCalled();

    // Wait for debounce
    vi.advanceTimersByTime(500);
    await act(async () => { await Promise.resolve(); });

    expect(commands.saveNewSessionDraft).toHaveBeenCalled();
  });

  it("calls saveNewSessionDraft after baseDir changes", async () => {
    vi.mocked(commands.getNewSessionHistorySummary).mockResolvedValue({
      recentWorkingDirs: [],
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "+ New" }));

    const baseDirInput = screen.getByPlaceholderText("e.g. /Users/you/projects/myapp");
    fireEvent.change(baseDirInput, { target: { value: "/new/dir" } });

    vi.advanceTimersByTime(500);
    await act(async () => { await Promise.resolve(); });

    expect(commands.saveNewSessionDraft).toHaveBeenCalled();
  });

  it("calls clearNewSessionDraft after sessionCreated event", async () => {
    vi.mocked(commands.saveNewSessionDraft).mockResolvedValue(undefined);

    // Given: createSession emits sessionCreated
    const control = setupTwoPhaseCreateSession("sess-draft-clear");

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "+ New" }));
    fireEvent.change(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      { target: { value: "task to generate" } }
    );
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    // When: sessionCreated fires
    await act(async () => {
      control.emitSessionCreated();
    });

    // Then: clearNewSessionDraft is called
    expect(commands.clearNewSessionDraft).toHaveBeenCalled();

    // Cleanup
    await act(async () => {
      control.emitPlanGenerated();
    });
  });
});

describe("App: New Session -- Recent Sessions section is absent", () => {
  beforeEach(setupNewSessionMocks);

  afterEach(() => {
    cleanup();
  });

  it("does not render 'Recent Sessions' heading", async () => {
    // When: user opens the New Session form
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await act(async () => { await new Promise<void>((r) => setTimeout(r, 50)); });

    // Then: "Recent Sessions" label is never rendered
    expect(screen.queryByText("Recent Sessions")).not.toBeInTheDocument();
  });

  it("still shows Working Directory quick-select chips (recentWorkingDirs preserved)", async () => {
    // Given: history summary provides recent working dirs (separate from Recent Sessions)
    vi.mocked(commands.getNewSessionHistorySummary).mockResolvedValue({
      recentWorkingDirs: ["/repos/alpha", "/repos/beta"],
    });

    // When: user opens the New Session form
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // Then: Working Directory chips are still rendered (not affected by this deletion)
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "/repos/alpha" })).toBeInTheDocument();
    });
    expect(screen.getByRole("button", { name: "/repos/beta" })).toBeInTheDocument();
    // And: "Recent Sessions" is still not shown
    expect(screen.queryByText("Recent Sessions")).not.toBeInTheDocument();
  });
});

describe("App: New Session skip-step selection", () => {
  beforeEach(setupNewSessionMocks);

  afterEach(() => {
    cleanup();
  });

  it("renders tree steps and applies history-based default skip selections on config refresh", async () => {
    vi.mocked(commands.getNewSessionConfigDefaults)
      .mockResolvedValueOnce({
        steps: BUILD_AND_REVIEW_STEPS,
        afterPrSteps: [],
        defaultSkippedSteps: ["build"],
      })
      .mockResolvedValueOnce({
        steps: BUILD_ONLY_STEP,
        afterPrSteps: [],
        defaultSkippedSteps: [],
      });

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    const buildCheckbox = await screen.findByRole("checkbox", { name: "build" });
    expect(buildCheckbox).toBeChecked();

    fireEvent.change(
      screen.getByPlaceholderText("e.g. /Users/you/projects/myapp"),
      { target: { value: "/tmp/other-repo" } }
    );

    await waitFor(() => {
      expect(commands.getNewSessionConfigDefaults).toHaveBeenLastCalledWith({
        baseDir: "/tmp/other-repo",
        configPath: undefined,
      });
    });
    expect(screen.getByRole("checkbox", { name: "build" })).not.toBeChecked();
  });

  it("refetches with new baseDir when baseDir changes under an explicit config (for history-scope defaults)", async () => {
    vi.mocked(commands.listConfigs).mockResolvedValue([
      { path: "/tmp/custom.yaml", name: "custom.yaml" },
    ]);
    vi.mocked(commands.getNewSessionConfigDefaults).mockResolvedValue({
      steps: BUILD_ONLY_STEP,
      afterPrSteps: [],
      defaultSkippedSteps: [],
    });

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.selectOptions(screen.getByLabelText("Config"), "/tmp/custom.yaml");
    await waitFor(() => expect(screen.getByLabelText("build")).toBeInTheDocument());

    const callCountBeforeBaseDirEdit =
      vi.mocked(commands.getNewSessionConfigDefaults).mock.calls.length;

    fireEvent.change(
      screen.getByPlaceholderText("e.g. /Users/you/projects/myapp"),
      { target: { value: "/tmp/new-worktree" } }
    );

    // baseDir is now always passed in directory mode so history-scope defaults
    // are fetched for the new directory, even when an explicit config is selected.
    await waitFor(() => {
      expect(commands.getNewSessionConfigDefaults).toHaveBeenLastCalledWith(
        expect.objectContaining({ baseDir: "/tmp/new-worktree", configPath: "/tmp/custom.yaml" })
      );
    });
    expect(vi.mocked(commands.getNewSessionConfigDefaults).mock.calls).toHaveLength(
      callCountBeforeBaseDirEdit + 1,
    );
  });
});

// --- Skip Steps following Config selector changes ----------------------------

describe("App: New Session skip-step selection -- Config change", () => {
  beforeEach(setupNewSessionMocks);

  afterEach(() => {
    cleanup();
  });

  it("replaces Skip Steps candidates with the new config's steps when switching explicit configs", async () => {
    // Given: two explicit configs -- configA has BUILD_AND_REVIEW_STEPS, configB has BUILD_ONLY_STEP
    vi.mocked(commands.listConfigs).mockResolvedValue([
      { path: "/tmp/configA.yaml", name: "configA.yaml" },
      { path: "/tmp/configB.yaml", name: "configB.yaml" },
    ]);
    vi.mocked(commands.getNewSessionConfigDefaults)
      .mockResolvedValueOnce({ steps: [], afterPrSteps: [], defaultSkippedSteps: [] })               // initial auto
      .mockResolvedValueOnce({ steps: BUILD_AND_REVIEW_STEPS, afterPrSteps: [], defaultSkippedSteps: ["build"] }) // select configA
      .mockResolvedValueOnce({ steps: BUILD_ONLY_STEP, afterPrSteps: [], defaultSkippedSteps: [] }); // select configB

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // When: select configA
    await userEvent.selectOptions(screen.getByLabelText("Config"), "/tmp/configA.yaml");

    // Then: configA's steps appear with "build" checked by default
    expect(await screen.findByRole("checkbox", { name: "build" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "review-pass" })).toBeInTheDocument();

    // When: switch to configB
    await userEvent.selectOptions(screen.getByLabelText("Config"), "/tmp/configB.yaml");

    // Then: only configB's steps are shown -- "review-pass" is gone, "build" is unchecked
    await waitFor(() => {
      expect(screen.queryByRole("checkbox", { name: "review-pass" })).not.toBeInTheDocument();
    });
    expect(screen.getByRole("checkbox", { name: "build" })).not.toBeChecked();
    expect(commands.getNewSessionConfigDefaults).toHaveBeenLastCalledWith({
      baseDir: ".",
      configPath: "/tmp/configB.yaml",
    });
  });

  it("updates Skip Steps when switching from auto mode to an explicit config", async () => {
    // Given: auto mode resolves BUILD_AND_REVIEW_STEPS; explicit config resolves BUILD_ONLY_STEP
    vi.mocked(commands.listConfigs).mockResolvedValue([
      { path: "/tmp/custom.yaml", name: "custom.yaml" },
    ]);
    vi.mocked(commands.getNewSessionConfigDefaults)
      .mockResolvedValueOnce({ steps: BUILD_AND_REVIEW_STEPS, afterPrSteps: [], defaultSkippedSteps: ["build"] }) // initial auto
      .mockResolvedValueOnce({ steps: BUILD_ONLY_STEP, afterPrSteps: [], defaultSkippedSteps: [] });               // select explicit

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // And: auto-mode steps are shown initially
    await screen.findByRole("checkbox", { name: "build" });
    expect(screen.getByRole("checkbox", { name: "review-pass" })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "build" })).toBeChecked();

    // When: user selects the explicit config
    await userEvent.selectOptions(screen.getByLabelText("Config"), "/tmp/custom.yaml");

    // Then: explicit config's steps replace the auto ones -- "review-pass" gone, "build" unchecked
    await waitFor(() => {
      expect(screen.queryByRole("checkbox", { name: "review-pass" })).not.toBeInTheDocument();
    });
    expect(screen.getByRole("checkbox", { name: "build" })).not.toBeChecked();
    expect(commands.getNewSessionConfigDefaults).toHaveBeenLastCalledWith({
      baseDir: ".",
      configPath: "/tmp/custom.yaml",
    });
  });

  it("restores auto-resolved Skip Steps when deselecting an explicit config back to Auto", async () => {
    // Given: explicit config has BUILD_ONLY_STEP; auto mode has BUILD_AND_REVIEW_STEPS
    vi.mocked(commands.listConfigs).mockResolvedValue([
      { path: "/tmp/custom.yaml", name: "custom.yaml" },
    ]);
    vi.mocked(commands.getNewSessionConfigDefaults)
      .mockResolvedValueOnce({ steps: [], afterPrSteps: [], defaultSkippedSteps: [] })                              // initial auto (no steps)
      .mockResolvedValueOnce({ steps: BUILD_ONLY_STEP, afterPrSteps: [], defaultSkippedSteps: [] })                  // select explicit
      .mockResolvedValueOnce({ steps: BUILD_AND_REVIEW_STEPS, afterPrSteps: [], defaultSkippedSteps: ["build"] });   // back to auto

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // Select explicit config
    await userEvent.selectOptions(screen.getByLabelText("Config"), "/tmp/custom.yaml");
    await screen.findByRole("checkbox", { name: "build" });
    expect(screen.queryByRole("checkbox", { name: "review-pass" })).not.toBeInTheDocument();

    // When: switch back to Auto
    await userEvent.selectOptions(screen.getByLabelText("Config"), "");

    // Then: auto-mode steps appear -- "review-pass" is back and "build" is checked by default
    await screen.findByRole("checkbox", { name: "review-pass" });
    expect(screen.getByRole("checkbox", { name: "build" })).toBeChecked();
    expect(commands.getNewSessionConfigDefaults).toHaveBeenLastCalledWith({
      baseDir: ".",
      configPath: undefined,
    });
  });

  it("does not carry over user-checked steps from the previous config when switching configs", async () => {
    // Given: configA has BUILD_AND_REVIEW_STEPS (no defaults); configB has BUILD_ONLY_STEP (no defaults)
    vi.mocked(commands.listConfigs).mockResolvedValue([
      { path: "/tmp/configA.yaml", name: "configA.yaml" },
      { path: "/tmp/configB.yaml", name: "configB.yaml" },
    ]);
    vi.mocked(commands.getNewSessionConfigDefaults)
      .mockResolvedValueOnce({ steps: [], afterPrSteps: [], defaultSkippedSteps: [] })                  // initial auto
      .mockResolvedValueOnce({ steps: BUILD_AND_REVIEW_STEPS, afterPrSteps: [], defaultSkippedSteps: [] }) // select configA
      .mockResolvedValueOnce({ steps: BUILD_ONLY_STEP, afterPrSteps: [], defaultSkippedSteps: [] });       // select configB

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // Select configA and manually check "build"
    await userEvent.selectOptions(screen.getByLabelText("Config"), "/tmp/configA.yaml");
    const buildCheckbox = await screen.findByRole("checkbox", { name: "build" });
    await userEvent.click(buildCheckbox);
    expect(buildCheckbox).toBeChecked();

    // When: switch to configB
    await userEvent.selectOptions(screen.getByLabelText("Config"), "/tmp/configB.yaml");

    // Then: configB's default (unchecked) is applied -- the manual check is not carried over
    await waitFor(() => {
      expect(screen.queryByRole("checkbox", { name: "review-pass" })).not.toBeInTheDocument();
    });
    expect(screen.getByRole("checkbox", { name: "build" })).not.toBeChecked();
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
  beforeEach(setupNewSessionMocks);

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
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

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
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

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
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

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

  it("refreshes recent working directories after sessionCreated", async () => {
    const control = setupTwoPhaseCreateSession("sess-history");
    vi.mocked(commands.getNewSessionHistorySummary)
      .mockResolvedValueOnce({
        recentWorkingDirs: ["/repos/old"],
      })
      .mockResolvedValueOnce({
        lastWorkingDir: "/repos/new",
        recentWorkingDirs: ["/repos/new", "/repos/old"],
      });

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    const baseDirInput = screen.getByPlaceholderText("e.g. /Users/you/projects/myapp");
    await userEvent.clear(baseDirInput);
    await userEvent.type(baseDirInput, "/repos/new");
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "history refresh",
    );
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    await act(async () => {
      control.emitSessionCreated();
    });

    await waitFor(() => {
      expect(screen.getByRole("button", { name: "/repos/new" })).toBeInTheDocument();
    });
    expect(screen.getByRole("button", { name: "/repos/old" })).toBeInTheDocument();

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
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

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
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

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
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

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
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

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

    // Select Session A -- Plan tab is the default
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

    // Select Session A -- Plan tab is the default and triggers initial loadPlan
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
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    // sessionCreated: session appears in sidebar in Draft+fixInProgress state (renders as "Planning" badge)
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-plan-ready", phase: "Draft", planAvailable: false, fixInProgress: true }),
    ]);
    await act(async () => { control.emitSessionCreated(); });
    await waitFor(() => {
      expect(screen.getByPlaceholderText("Describe what you want to implement...")).toHaveValue("");
    });

    // When: planGenerated fires -> session becomes approval-ready (planAvailable: true)
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
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    // When: sessionCreated fires -> session is Draft+fixInProgress (renders as "Planning" badge)
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-planning", phase: "Draft", planAvailable: false, fixInProgress: true }),
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

    // Then: Plan tab is active by default -- Info tab's "Base dir" label is not visible
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
    // And: getSessionPlan was NOT called -- plan is not available yet
    expect(commands.getSessionPlan).not.toHaveBeenCalled();
  });

  it("auto-loads plan when open session transitions from planAvailable: false to planAvailable: true", async () => {
    // Given: session starts without a plan; Plan tab is the default
    const sessV1 = makeSession({ id: "sess-late-plan", planAvailable: false });
    vi.mocked(commands.listSessions).mockResolvedValue([sessV1]);
    vi.mocked(commands.getSessionPlan).mockResolvedValue("# Late plan");

    render(<App />);
    await waitFor(() => screen.getByText("test task"));

    // Select the session -- Plan tab is shown but plan is not loaded
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

    // Open session A -- Plan tab is the default
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

    // Then: Log tab is still active for session A -- the remembered tab wins over the default
    await waitFor(() => {
      expect(screen.getByRole("tab", { name: "Log" })).toHaveAttribute("aria-selected", "true");
    });
    expect(screen.getByRole("tab", { name: "Plan" })).toHaveAttribute("aria-selected", "false");
  });
});

// --- useInputAsPlan checkbox --------------------------------------------------

describe("App: New Session -- useInputAsPlan checkbox", () => {
  beforeEach(setupNewSessionMocks);

  afterEach(() => {
    cleanup();
  });

  it("renders 'Use input as plan' checkbox unchecked by default", async () => {
    // Given: New Session form is opened
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // When: the form renders
    const checkbox = await screen.findByRole("checkbox", {
      name: /use input as plan/i,
    });

    // Then: checkbox is unchecked by default
    expect(checkbox).not.toBeChecked();
  });

  it("shows 'Generate plan' button label when useInputAsPlan is unchecked (default)", async () => {
    // Given: New Session form is opened
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // When: checkbox is in unchecked state (default)
    await screen.findByRole("checkbox", { name: /use input as plan/i });

    // Then: submit button shows "Generate plan"
    expect(screen.getByRole("button", { name: "Generate plan" })).toBeInTheDocument();
  });

  it("changes submit button label to 'Create session' when useInputAsPlan is checked", async () => {
    // Given: New Session form is opened
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    const checkbox = await screen.findByRole("checkbox", { name: /use input as plan/i });

    // When: user checks the checkbox
    await userEvent.click(checkbox);

    // Then: submit button label changes to "Create session"
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Create session" })).toBeInTheDocument();
    });
    expect(screen.queryByRole("button", { name: "Generate plan" })).not.toBeInTheDocument();
  });

  it("reverts submit button label to 'Generate plan' when useInputAsPlan is unchecked again", async () => {
    // Given: checkbox is checked
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    const checkbox = await screen.findByRole("checkbox", { name: /use input as plan/i });
    await userEvent.click(checkbox);
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Create session" })).toBeInTheDocument();
    });

    // When: user unchecks the checkbox
    await userEvent.click(checkbox);

    // Then: submit button label reverts to "Generate plan"
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Generate plan" })).toBeInTheDocument();
    });
    expect(screen.queryByRole("button", { name: "Create session" })).not.toBeInTheDocument();
  });

  it("passes useInputAsPlan: true to createSession when checkbox is checked", async () => {
    // Given: createSession mock that resolves immediately
    const control = setupTwoPhaseCreateSession("sess-skip-plan");
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // Type a task
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "my direct plan"
    );

    // Check the checkbox
    const checkbox = await screen.findByRole("checkbox", { name: /use input as plan/i });
    await userEvent.click(checkbox);

    // When: submit the form
    fireEvent.click(screen.getByRole("button", { name: "Create session" }));

    // Then: createSession is called with useInputAsPlan: true
    expect(commands.createSession).toHaveBeenCalledWith(
      expect.objectContaining({ useInputAsPlan: true }),
      expect.anything()
    );

    // Cleanup
    await act(async () => { control.emitSessionCreated(); });
    await act(async () => { control.emitPlanGenerated(); });
  });

  it("passes useInputAsPlan: false to createSession when checkbox is unchecked (default)", async () => {
    // Given: createSession mock
    const control = setupTwoPhaseCreateSession("sess-with-llm");
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // Type a task without touching the checkbox
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "my planned task"
    );

    // When: submit with default unchecked checkbox
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    // Then: createSession is called with useInputAsPlan: false
    expect(commands.createSession).toHaveBeenCalledWith(
      expect.objectContaining({ useInputAsPlan: false }),
      expect.anything()
    );

    // Cleanup
    await act(async () => { control.emitSessionCreated(); });
    await act(async () => { control.emitPlanGenerated(); });
  });

  it("checkbox is disabled while session creation is in progress", async () => {
    // Given: createSession is pending (session creation in progress)
    const control = setupTwoPhaseCreateSession("sess-pending");
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "task"
    );
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    // When: session creation is in progress (before sessionCreated fires)
    // Then: the checkbox is disabled
    const checkbox = await screen.findByRole("checkbox", { name: /use input as plan/i });
    expect(checkbox).toBeDisabled();

    // Cleanup
    await act(async () => { control.emitSessionCreated(); });
    await act(async () => { control.emitPlanGenerated(); });
  });

  it("does not persist useInputAsPlan in saveNewSessionDraft", async () => {
    // Given: form is open
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "+ New" }));

    // Check the checkbox (wait with real timers so findByRole can poll)
    const checkbox = await screen.findByRole("checkbox", { name: /use input as plan/i });
    fireEvent.click(checkbox);

    // Switch to fake timers for debounce testing and clear prior calls
    vi.useFakeTimers();
    vi.mocked(commands.saveNewSessionDraft).mockClear();

    // Trigger debounced save via task input change
    fireEvent.change(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      { target: { value: "my task" } }
    );
    vi.advanceTimersByTime(500);
    await act(async () => { await Promise.resolve(); });

    // Then: saveNewSessionDraft is called WITHOUT a useInputAsPlan field
    expect(commands.saveNewSessionDraft).toHaveBeenCalledWith(
      expect.not.objectContaining({ useInputAsPlan: expect.anything() })
    );

    vi.useRealTimers();
  });
});

// --- listConfigs reactive refetch on source change ----------------------------

describe("App: New Session -- listConfigs reacts to source changes", () => {
  beforeEach(() => {
    setupNewSessionMocks();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    cleanup();
  });

  it("calls listConfigs with { baseDir } when working directory is set on mount", async () => {
    // Given: history provides a last working directory
    vi.mocked(commands.getNewSessionHistorySummary).mockResolvedValue({
      lastWorkingDir: "/initial/project",
      recentWorkingDirs: ["/initial/project"],
    });

    // When: the form is opened and async effects settle
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "+ New" }));
    await act(async () => { await Promise.resolve(); });
    await act(async () => { await Promise.resolve(); });
    vi.advanceTimersByTime(300); // allow debounce to fire
    await act(async () => { await Promise.resolve(); });

    // Then: listConfigs is called with a baseDir argument
    expect(commands.listConfigs).toHaveBeenCalledWith(
      expect.objectContaining({ baseDir: expect.any(String) })
    );
  });

  it("re-calls listConfigs with new baseDir after working directory input changes (debounced)", async () => {
    // Given: form is open with initial baseDir
    vi.mocked(commands.getNewSessionHistorySummary).mockResolvedValue({
      lastWorkingDir: "/old/project",
      recentWorkingDirs: [],
    });
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "+ New" }));
    await act(async () => { await Promise.resolve(); });
    await act(async () => { await Promise.resolve(); });
    vi.advanceTimersByTime(300);
    await act(async () => { await Promise.resolve(); });
    vi.mocked(commands.listConfigs).mockClear();

    // When: user types a new working directory
    const baseDirInput = screen.getByPlaceholderText("e.g. /Users/you/projects/myapp");
    fireEvent.change(baseDirInput, { target: { value: "/new/project" } });

    // Not called yet (debounce)
    expect(commands.listConfigs).not.toHaveBeenCalled();

    // After debounce fires
    vi.advanceTimersByTime(300);
    await act(async () => { await Promise.resolve(); });

    // Then: listConfigs is called with the new baseDir
    expect(commands.listConfigs).toHaveBeenCalledWith(
      expect.objectContaining({ baseDir: "/new/project" })
    );
  });

  it("re-calls listConfigs immediately when source switches to repository mode", async () => {
    // Given: form is open in directory mode
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "+ New" }));
    await act(async () => { await Promise.resolve(); });
    await act(async () => { await Promise.resolve(); });
    vi.advanceTimersByTime(300);
    await act(async () => { await Promise.resolve(); });
    vi.mocked(commands.listConfigs).mockClear();

    // When: user switches to repository mode
    const repoRadio = screen.getByRole("radio", { name: "GitHub Repository" });
    fireEvent.click(repoRadio);
    await act(async () => { await Promise.resolve(); });

    // Then: listConfigs is called again (isRepoMode change is not debounced)
    expect(commands.listConfigs).toHaveBeenCalled();
  });

  it("resets configPath to '' when the selected config disappears from the updated list", async () => {
    // Given: form opens with a selected config, listConfigs initially returns that config
    vi.mocked(commands.listConfigs).mockResolvedValue([
      { path: "/my/project/cruise.yaml", name: "cruise.yaml" },
    ]);
    vi.mocked(commands.getNewSessionHistorySummary).mockResolvedValue({
      lastRequestedConfigPath: "/my/project/cruise.yaml",
      lastWorkingDir: "/my/project",
      recentWorkingDirs: [],
    });

    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "+ New" }));
    await act(async () => { await Promise.resolve(); });
    await act(async () => { await Promise.resolve(); });
    vi.advanceTimersByTime(300);
    await act(async () => { await Promise.resolve(); });
    await act(async () => { await Promise.resolve(); });

    // Config should be selected from history; configs list loaded from initial immediate call.
    expect(screen.getByLabelText("Config")).toHaveValue("/my/project/cruise.yaml");

    // When: working directory changes and the new list no longer contains the selected config
    vi.mocked(commands.listConfigs).mockResolvedValue([]);
    const baseDirInput = screen.getByPlaceholderText("e.g. /Users/you/projects/myapp");
    fireEvent.change(baseDirInput, { target: { value: "/other/project" } });
    vi.advanceTimersByTime(300);
    await act(async () => { await Promise.resolve(); });
    await act(async () => { await Promise.resolve(); });

    // Then: config select is reset to "" (Auto)
    expect(screen.getByLabelText("Config")).toHaveValue("");
  });
});

// --- Planning badge during create_session (Draft + FixingGuard flow) ----------
//
// These tests document the fix for: GUI shows "Awaiting Approval" during LLM
// plan generation instead of "Planning".
//
// After the fix, create_session (useInputAsPlan=false) persists the session as
// Draft + starts a FixingGuard before sending SessionCreated.  The sidebar
// therefore sees phase:"Draft" + fixInProgress:true → PhaseBadge renders
// "Planning".  On success, the backend promotes to AwaitingApproval before
// emitting PlanGenerated.

describe("App: Planning badge during plan generation (create_session)", () => {
  beforeEach(setupNewSessionMocks);

  afterEach(() => {
    cleanup();
  });

  it("shows 'Planning' badge in sidebar immediately after sessionCreated while plan is generating", async () => {
    // Given: createSession will emit sessionCreated before planGenerated
    const control = setupTwoPhaseCreateSession("sess-planning-badge");

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "plan in progress"
    );
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    // When: sessionCreated fires -- backend has persisted Draft+FixingGuard
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-planning-badge", phase: "Draft", planAvailable: false, fixInProgress: true }),
    ]);
    await act(async () => { control.emitSessionCreated(); });

    // Then: sidebar shows "Planning" (not "Awaiting Approval")
    await waitFor(() => expect(screen.getByText("Planning")).toBeInTheDocument());
    expect(screen.queryByText("Awaiting Approval")).not.toBeInTheDocument();

    // Cleanup
    await act(async () => { control.emitPlanGenerated(); });
  });

  it("transitions sidebar badge from 'Planning' to 'Awaiting Approval' when planGenerated fires", async () => {
    // Given: createSession flow with two phases
    const control = setupTwoPhaseCreateSession("sess-badge-transition");

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "badge transition task"
    );
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    // sessionCreated: Draft + fixInProgress → "Planning"
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-badge-transition", phase: "Draft", planAvailable: false, fixInProgress: true }),
    ]);
    await act(async () => { control.emitSessionCreated(); });
    await waitFor(() => expect(screen.getByText("Planning")).toBeInTheDocument());

    // When: planGenerated fires -- backend promotes to AwaitingApproval
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-badge-transition", phase: "Awaiting Approval", planAvailable: true }),
    ]);
    await act(async () => { control.emitPlanGenerated(); });

    // Then: badge updates to "Awaiting Approval" (plan is now ready for approval)
    await waitFor(() => expect(screen.getByText("Awaiting Approval")).toBeInTheDocument());
    expect(screen.queryByText("Planning")).not.toBeInTheDocument();
  });

  it("does not show 'Planning' badge when useInputAsPlan is true (synchronous -- bypasses Draft)", async () => {
    // Given: createSession called with useInputAsPlan: true
    const control = setupTwoPhaseCreateSession("sess-input-as-plan");

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // Enable useInputAsPlan
    const checkbox = await screen.findByRole("checkbox", { name: /use input as plan/i });
    await userEvent.click(checkbox);

    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "direct plan content"
    );
    fireEvent.click(screen.getByRole("button", { name: "Create session" }));

    // sessionCreated: for useInputAsPlan the backend writes the plan synchronously
    // and stays in AwaitingApproval (no Draft phase, no FixingGuard)
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-input-as-plan", phase: "Awaiting Approval", planAvailable: true }),
    ]);
    await act(async () => { control.emitSessionCreated(); });

    // Then: badge shows "Awaiting Approval" directly (no "Planning" intermediate state)
    await waitFor(() => expect(screen.getByText("Awaiting Approval")).toBeInTheDocument());
    expect(screen.queryByText("Planning")).not.toBeInTheDocument();

    // Cleanup
    await act(async () => { control.emitPlanGenerated(); });
  });

  it("'Planning' badge disappears from sidebar after planFailed (session is deleted by backend)", async () => {
    // Given: plan generation fails
    const control = setupTwoPhaseCreateSession("sess-plan-fail-badge");

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.type(
      screen.getByPlaceholderText("Describe what you want to implement..."),
      "failing plan"
    );
    fireEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    // sessionCreated: Draft + fixInProgress → "Planning"
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "sess-plan-fail-badge", phase: "Draft", planAvailable: false, fixInProgress: true }),
    ]);
    await act(async () => { control.emitSessionCreated(); });
    await waitFor(() => expect(screen.getByText("Planning")).toBeInTheDocument());

    // When: planFailed fires -- backend deletes the session
    vi.mocked(commands.listSessions).mockResolvedValue([]);
    await act(async () => { control.emitPlanFailed("model error"); });

    // Then: "Planning" badge (and the session row) disappears from the sidebar
    await waitFor(() => expect(screen.queryByText("Planning")).not.toBeInTheDocument());
  });
});

// --- Skip Steps defaults in repository mode -----------------------------------

describe("App: New Session skip-step defaults -- repository mode", () => {
  beforeEach(setupNewSessionMocks);

  afterEach(() => {
    cleanup();
  });

  it("passes repo to getNewSessionConfigDefaults when in repo mode and repo is typed", async () => {
    // Given: form is open and switched to repo mode
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.click(screen.getByRole("radio", { name: "GitHub Repository" }));
    vi.mocked(commands.getNewSessionConfigDefaults).mockClear();

    // When: user types a valid repo spec
    const repoInput = screen.getByLabelText("Repository");
    fireEvent.change(repoInput, { target: { value: "owner/myrepo" } });

    // Then: getNewSessionConfigDefaults is called with repo included
    await waitFor(() => {
      expect(commands.getNewSessionConfigDefaults).toHaveBeenCalledWith(
        expect.objectContaining({ repo: "owner/myrepo" })
      );
    });
  });

  it("refetches config defaults when switching from directory mode to repo mode", async () => {
    // Given: form is open in directory mode, initial call has fired
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await waitFor(() => expect(commands.getNewSessionConfigDefaults).toHaveBeenCalled());
    vi.mocked(commands.getNewSessionConfigDefaults).mockClear();

    // When: user switches to repo mode
    await userEvent.click(screen.getByRole("radio", { name: "GitHub Repository" }));

    // Then: getNewSessionConfigDefaults fires again (isRepoMode change triggers the useEffect)
    await waitFor(() => {
      expect(commands.getNewSessionConfigDefaults).toHaveBeenCalled();
    });
  });

  it("refetches config defaults when switching back from repo mode to directory mode", async () => {
    // Given: form is in repo mode
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));
    await userEvent.click(screen.getByRole("radio", { name: "GitHub Repository" }));
    await waitFor(() => expect(commands.getNewSessionConfigDefaults).toHaveBeenCalled());
    vi.mocked(commands.getNewSessionConfigDefaults).mockClear();

    // When: user switches back to directory mode
    await userEvent.click(screen.getByRole("radio", { name: "Directory" }));

    // Then: getNewSessionConfigDefaults fires again, and repo is NOT included
    await waitFor(() => {
      expect(commands.getNewSessionConfigDefaults).toHaveBeenCalled();
    });
    const lastCall = vi.mocked(commands.getNewSessionConfigDefaults).mock.calls.at(-1)?.[0];
    expect(lastCall).not.toHaveProperty("repo");
  });
});

// --- New Session form DOM order -----------------------------------------------

describe("App: New Session -- form section order", () => {
  beforeEach(setupNewSessionMocks);

  afterEach(() => {
    cleanup();
  });

  it("renders Source section before Config section in the form", async () => {
    // When: the New Session form is open
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // Then: the Source radiogroup appears before the Config select in document order
    const sourceSection = screen.getByRole("radiogroup", { name: "Workspace source" });
    const configSelect = screen.getByLabelText("Config");
    const order = sourceSection.compareDocumentPosition(configSelect);
    expect(order & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("renders Working Directory input before Config section in the form", async () => {
    // When: the New Session form is open in directory mode (default)
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // Then: the Working Directory input appears before the Config select in document order
    const baseDirInput = screen.getByPlaceholderText("e.g. /Users/you/projects/myapp");
    const configSelect = screen.getByLabelText("Config");
    const order = baseDirInput.compareDocumentPosition(configSelect);
    expect(order & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("renders Config section before Task input in the form", async () => {
    // When: the New Session form is open
    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    // Then: the Config select appears before the Task textarea in document order
    const configSelect = screen.getByLabelText("Config");
    const taskInput = screen.getByPlaceholderText("Describe what you want to implement...");
    const order = configSelect.compareDocumentPosition(taskInput);
    expect(order & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});
