import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup, fireEvent, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "../App";
import type { Session, PlanEvent } from "../types";
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
  updateSessionSettings: vi.fn(),
  regenerateSessionPlan: vi.fn(),
}));

vi.mock("../lib/updater", () => ({
  checkForUpdate: vi.fn().mockResolvedValue(null),
  checkForUpdateManual: vi.fn().mockResolvedValue(null),
  downloadAndInstall: vi.fn(),
}));

vi.mock("../lib/desktopNotifications", () => ({
  notifyDesktop: vi.fn(),
}));

function makeSession(overrides: Partial<Session> = {}): Session {
  return {
    id: "session-1",
    phase: "Awaiting Approval",
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

const SKIP_STEPS = [
  { id: "build", expandedStepIds: ["build"], children: [] },
  { id: "test", expandedStepIds: ["test"], children: [] },
  { id: "review", expandedStepIds: ["review"], children: [] },
];

describe("App: Post-plan session editing - UI visibility", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listConfigs).mockResolvedValue([
      { name: "default.yaml", path: "/Users/takumi/.cruise/default.yaml" },
      { name: "minimal.yaml", path: "/Users/takumi/.cruise/minimal.yaml" },
    ]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("# Plan content");
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
  });

  afterEach(() => {
    cleanup();
  });

  it("shows session settings editor for Awaiting Approval session", async () => {
    const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);
    vi.mocked(commands.getSession).mockResolvedValue(session);

    render(<App />);
    await waitFor(() => screen.getByText("test task"));
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    await waitFor(() => {
      expect(screen.getByText("Session Settings")).toBeInTheDocument();
    });
  });

  it("shows session settings editor for Planned session", async () => {
    const session = makeSession({ phase: "Planned", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);
    vi.mocked(commands.getSession).mockResolvedValue(session);

    render(<App />);
    await waitFor(() => screen.getByText("test task"));
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    await waitFor(() => {
      expect(screen.getByText("Session Settings")).toBeInTheDocument();
    });
  });

  it("does not show session settings editor for Running session", async () => {
    const session = makeSession({ phase: "Running", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);
    vi.mocked(commands.getSession).mockResolvedValue(session);

    render(<App />);
    await waitFor(() => screen.getByText("test task"));
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    await waitFor(() => {
      expect(screen.queryByText("Session Settings")).toBeNull();
    });
  });

  it("does not show session settings editor for Suspended session", async () => {
    const session = makeSession({ phase: "Suspended", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);
    vi.mocked(commands.getSession).mockResolvedValue(session);

    render(<App />);
    await waitFor(() => screen.getByText("test task"));
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    await waitFor(() => {
      expect(screen.queryByText("Session Settings")).toBeNull();
    });
  });

  it("does not show session settings editor for Failed session", async () => {
    const session = makeSession({ phase: "Failed", planAvailable: true, phaseError: "Step 2 failed" });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);
    vi.mocked(commands.getSession).mockResolvedValue(session);

    render(<App />);
    await waitFor(() => screen.getByText("test task"));
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    await waitFor(() => {
      expect(screen.queryByText("Session Settings")).toBeNull();
    });
  });

  it("does not show session settings editor for Completed session", async () => {
    const session = makeSession({ phase: "Completed", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);
    vi.mocked(commands.getSession).mockResolvedValue(session);

    render(<App />);
    await waitFor(() => screen.getByText("test task"));
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    await waitFor(() => {
      expect(screen.queryByText("Session Settings")).toBeNull();
    });
  });
});

describe("App: Post-plan session editing - skip-only save", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listConfigs).mockResolvedValue([
      { name: "default.yaml", path: "/Users/takumi/.cruise/default.yaml" },
    ]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("# Plan content");
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
    vi.mocked(commands.getNewSessionConfigDefaults).mockResolvedValue({
      steps: SKIP_STEPS,
      defaultSkippedSteps: [],
    });
  });

  afterEach(() => {
    cleanup();
  });

  it("calls updateSessionSettings when saving skip-only changes", async () => {
    vi.mocked(commands.updateSessionSettings).mockResolvedValue({
      ...makeSession(),
      skippedSteps: ["build"],
    });

    const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);
    vi.mocked(commands.getSession).mockResolvedValue(session);

    render(<App />);
    await waitFor(() => screen.getByText("test task"));
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    await waitFor(() => {
      expect(screen.getByText("Session Settings")).toBeInTheDocument();
    });

    const buildCheckbox = await screen.findByRole("checkbox", { name: "build" });
    await userEvent.click(buildCheckbox);

    const saveButton = screen.getByRole("button", { name: "Save" });
    await userEvent.click(saveButton);

    await waitFor(() => {
      expect(commands.updateSessionSettings).toHaveBeenCalledWith(
        "session-1",
        expect.objectContaining({
          skippedSteps: ["build"],
        })
      );
    });
  });

  it("does not call regenerateSessionPlan when saving skip-only changes", async () => {
    vi.mocked(commands.updateSessionSettings).mockResolvedValue({
      ...makeSession(),
      skippedSteps: ["build"],
    });

    const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);
    vi.mocked(commands.getSession).mockResolvedValue(session);

    render(<App />);
    await waitFor(() => screen.getByText("test task"));
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    await waitFor(() => {
      expect(screen.getByText("Session Settings")).toBeInTheDocument();
    });

    const buildCheckbox = await screen.findByRole("checkbox", { name: "build" });
    await userEvent.click(buildCheckbox);

    const saveButton = screen.getByRole("button", { name: "Save" });
    await userEvent.click(saveButton);

    await waitFor(() => {
      expect(commands.regenerateSessionPlan).not.toHaveBeenCalled();
    });
  });

  it("reflects updated skippedSteps immediately in session.skippedSteps after save", async () => {
    const savedSession = {
      ...makeSession(),
      skippedSteps: ["build"],
    };
    vi.mocked(commands.updateSessionSettings).mockResolvedValue(savedSession);

    const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);
    vi.mocked(commands.getSession)
      .mockResolvedValueOnce(session)
      .mockResolvedValueOnce(savedSession);

    render(<App />);
    await waitFor(() => screen.getByText("test task"));
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    await waitFor(() => {
      expect(screen.getByText("Session Settings")).toBeInTheDocument();
    });

    const buildCheckbox = await screen.findByRole("checkbox", { name: "build" });
    await userEvent.click(buildCheckbox);

    const saveButton = screen.getByRole("button", { name: "Save" });
    await userEvent.click(saveButton);

    await waitFor(() => {
      expect(commands.updateSessionSettings).toHaveBeenCalled();
    });

    await waitFor(() => {
      expect(buildCheckbox).toBeChecked();
    });
  });
});

describe("App: Post-plan session editing - config change with replan", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listConfigs).mockResolvedValue([
      { name: "default.yaml", path: "/Users/takumi/.cruise/default.yaml" },
      { name: "minimal.yaml", path: "/Users/takumi/.cruise/minimal.yaml" },
    ]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("# Old plan content");
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
    vi.mocked(commands.getNewSessionConfigDefaults).mockResolvedValue({
      steps: SKIP_STEPS,
      defaultSkippedSteps: [],
    });
  });

  afterEach(() => {
    cleanup();
  });

  it("calls regenerateSessionPlan after saving config change", async () => {
    vi.mocked(commands.updateSessionSettings).mockResolvedValue({
      ...makeSession(),
      configSource: "minimal.yaml",
    });
    vi.mocked(commands.regenerateSessionPlan).mockImplementationOnce(
      async () => "# New plan content"
    );

    const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);
    vi.mocked(commands.getSession).mockResolvedValue(session);

    render(<App />);
    await waitFor(() => screen.getByText("test task"));
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    await waitFor(() => {
      expect(screen.getByText("Session Settings")).toBeInTheDocument();
    });

    const configSelect = screen.getByLabelText("Config");
    fireEvent.change(configSelect, { target: { value: "/Users/takumi/.cruise/minimal.yaml" } });

    const saveButton = await screen.findByRole("button", { name: "Save & Regenerate Plan" });
    await userEvent.click(saveButton);

    await waitFor(() => {
      expect(commands.regenerateSessionPlan).toHaveBeenCalledWith(
        "session-1",
        expect.anything()
      );
    });
  });

  it("updates Plan tab content after plan regeneration completes", async () => {
    const updatedSession = {
      ...makeSession(),
      configSource: "minimal.yaml",
    };
    vi.mocked(commands.updateSessionSettings).mockResolvedValue(updatedSession);
    vi.mocked(commands.regenerateSessionPlan).mockImplementationOnce(
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      async (_sessionId: any, channel: any) => {
        channel.onmessage?.({
          event: "planGenerated",
          data: { content: "# New plan content" },
        });
        return "# New plan content";
      }
    );
    vi.mocked(commands.getSession).mockResolvedValue(updatedSession);
    vi.mocked(commands.getSessionPlan).mockResolvedValue("# New plan content");

    const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);

    render(<App />);
    await waitFor(() => screen.getByText("test task"));
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    await waitFor(() => {
      expect(screen.getByText("Session Settings")).toBeInTheDocument();
    });

    const configSelect = screen.getByLabelText("Config");
    fireEvent.change(configSelect, { target: { value: "/Users/takumi/.cruise/minimal.yaml" } });

    const saveButton = await screen.findByRole("button", { name: "Save & Regenerate Plan" });
    await userEvent.click(saveButton);

    await waitFor(() => {
      expect(screen.getByText("New plan content")).toBeInTheDocument();
    });
  });

  it("disables action buttons while plan regeneration is in progress", async () => {
    vi.mocked(commands.updateSessionSettings).mockResolvedValue({
      ...makeSession(),
      configSource: "minimal.yaml",
    });
    vi.mocked(commands.regenerateSessionPlan).mockImplementationOnce(
      async () => new Promise<string>(() => {})
    );

    const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);
    vi.mocked(commands.getSession).mockResolvedValue(session);

    render(<App />);
    await waitFor(() => screen.getByText("test task"));
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    await waitFor(() => {
      expect(screen.getByText("Session Settings")).toBeInTheDocument();
    });

    const configSelect = screen.getByLabelText("Config");
    fireEvent.change(configSelect, { target: { value: "/Users/takumi/.cruise/minimal.yaml" } });

    const saveButton = await screen.findByRole("button", { name: "Save & Regenerate Plan" });
    await userEvent.click(saveButton);

    await waitFor(() => {
      expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
      expect(screen.queryByRole("button", { name: "Fix" })).toBeNull();
      expect(screen.queryByRole("button", { name: "Ask" })).toBeNull();
    });
  });
});

describe("App: Post-plan session editing - repo-local config handling", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listConfigs).mockResolvedValue([
      { name: "default.yaml", path: "/Users/takumi/.cruise/default.yaml" },
    ]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("# Plan content");
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
    vi.mocked(commands.getNewSessionConfigDefaults).mockResolvedValue({
      steps: SKIP_STEPS,
      defaultSkippedSteps: [],
    });
  });

  afterEach(() => {
    cleanup();
  });

  it("initializes editor correctly when session has a repo-local config not in listConfigs", async () => {
    const session = makeSession({
      phase: "Awaiting Approval",
      planAvailable: true,
      configSource: "/repos/myproject/cruise.yaml",
      configPath: "/repos/myproject/cruise.yaml",
    });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);
    vi.mocked(commands.getSession).mockResolvedValue(session);

    render(<App />);
    await waitFor(() => screen.getByText("test task"));
    await userEvent.click(screen.getByRole("button", { name: /test task/ }));

    await waitFor(() => {
      expect(screen.getByText("Session Settings")).toBeInTheDocument();
    });

    const configInput = screen.getByLabelText("Config");
    expect(configInput).toHaveValue("/repos/myproject/cruise.yaml");
  });
});

describe("App: Post-plan session editing - existing New Session behavior preserved", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listSessions).mockResolvedValue([]);
    vi.mocked(commands.listConfigs).mockResolvedValue([]);
    vi.mocked(commands.getSessionLog).mockResolvedValue("");
    vi.mocked(commands.getSessionPlan).mockResolvedValue("");
    vi.mocked(commands.getNewSessionHistorySummary).mockResolvedValue({ recentWorkingDirs: [] });
    vi.mocked(commands.getNewSessionConfigDefaults).mockResolvedValue({
      steps: [],
      defaultSkippedSteps: [],
    });
    vi.mocked(commands.listDirectory).mockResolvedValue([]);
    vi.mocked(commands.getUpdateReadiness).mockResolvedValue({ canAutoUpdate: true });
    vi.mocked(commands.cleanSessions).mockResolvedValue({ deleted: 0, skipped: 0 });
  });

  afterEach(() => {
    cleanup();
  });

  it("releases New Session form when sessionCreated event is received", async () => {
    let capturedChannel: { onmessage: ((event: PlanEvent) => void) | null } | null = null;
    vi.mocked(commands.createSession).mockImplementationOnce(
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (_params: any, channel: any) => {
        capturedChannel = channel;
        return new Promise<string>(() => {});
      }
    );

    render(<App />);
    await userEvent.click(screen.getByRole("button", { name: "+ New" }));

    const taskTextarea = screen.getByPlaceholderText("Describe what you want to implement...");
    await userEvent.type(taskTextarea, "my new task");

    await userEvent.click(screen.getByRole("button", { name: "Generate plan" }));

    await waitFor(() => {
      expect(capturedChannel).not.toBeNull();
    });

    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "new-session-id", input: "my new task", phase: "Planned" }),
    ]);

    await act(async () => {
      capturedChannel?.onmessage?.({ event: "sessionCreated", data: { sessionId: "new-session-id" } });
    });

    await waitFor(() => {
      expect(taskTextarea).toHaveValue("");
    });
  });
});
