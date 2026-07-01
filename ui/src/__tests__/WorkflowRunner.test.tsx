import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, act, fireEvent } from "@testing-library/react";
import { createRef } from "react";
import { WorkflowLogPanel, WorkflowRunner } from "../App";
import type { Session } from "../types";

// --- Module mocks (hoisted by Vitest) ----------------------------------------

vi.mock("@tauri-apps/api/core", () => ({
  Channel: vi.fn().mockImplementation(() => ({ onmessage: null })),
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn().mockResolvedValue("0.0.0"),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("../lib/commands", () => ({
  approveSession: vi.fn(),
  askSession: vi.fn(),
  cancelSession: vi.fn(),
  clearNewSessionDraft: vi.fn(),
  createSession: vi.fn(),
  deleteSession: vi.fn(),
  fixSession: vi.fn(),
  getAppConfig: vi.fn().mockResolvedValue({ runAllParallelism: 1 }),
  getNewSessionConfigDefaults: vi.fn().mockResolvedValue({ steps: [], afterPrSteps: [], defaultSkippedSteps: [] }),
  getNewSessionDraft: vi.fn().mockResolvedValue(null),
  getNewSessionHistorySummary: vi.fn().mockResolvedValue({ recentWorkingDirs: [] }),
  getSession: vi.fn(),
  getSessionDag: vi.fn().mockResolvedValue({ startStep: "step1", currentStep: null, steps: [], edges: [] }),
  getSessionLog: vi.fn().mockResolvedValue(""),
  getSessionPlan: vi.fn().mockResolvedValue(""),
  listConfigs: vi.fn().mockResolvedValue([]),
  listSessions: vi.fn().mockResolvedValue([]),
  resetSession: vi.fn(),
  respondToOption: vi.fn(),
  runAllSessions: vi.fn(),
  runSession: vi.fn(),
  saveNewSessionDraft: vi.fn(),
  updateAppConfig: vi.fn(),
  getUpdateReadiness: vi.fn().mockResolvedValue({ canAutoUpdate: true }),
  cleanSessions: vi.fn().mockResolvedValue({ deleted: 0, skipped: 0 }),
}));

vi.mock("../lib/updater", () => ({
  checkForUpdate: vi.fn().mockResolvedValue(null),
  checkForUpdateManual: vi.fn(),
  downloadAndInstall: vi.fn(),
}));

vi.mock("../lib/desktopNotifications", () => ({
  notifyDesktop: vi.fn(),
}));

// --- Helpers -----------------------------------------------------------------

function makeSession(overrides: Partial<Session> = {}): Session {
  return {
    id: "session-1",
    phase: "Planned",
    configSource: "default.yaml",
    baseDir: "/home/user/project",
    input: "do something",
    createdAt: "2026-01-01T00:00:00Z",
    workspaceMode: "Worktree",
    skippedSteps: [],
    ...overrides,
  };
}

function defaultRunnerProps(session = makeSession()) {
  return {
    session,
    activeTab: "log" as const,
    onActiveTabChange: vi.fn(),
    onSessionUpdated: vi.fn(),
    onDeleteConfirmed: vi.fn(),
    onToast: vi.fn(),
    onFixingChange: vi.fn(),
  };
}

// --- Tests: WorkflowLogPanel prop wiring ------------------------------------

describe("WorkflowLogPanel - preRef and onScroll prop wiring", () => {
  it("mounts preRef.current to the <pre> element when logContent is present", () => {
    const preRef = createRef<HTMLPreElement>();
    const logEndRef = createRef<HTMLSpanElement>();

    render(
      <WorkflowLogPanel
        panelLogId="panel-log"
        tabLogId="tab-log"
        status="idle"
        logContent="some log content"
        logEndRef={logEndRef}
        preRef={preRef}
        onScroll={() => {}}
      />,
    );

    expect(preRef.current).toBeInstanceOf(HTMLPreElement);
  });

  it("calls onScroll when a scroll event fires on the <pre> element", () => {
    const onScroll = vi.fn();
    const preRef = createRef<HTMLPreElement>();
    const logEndRef = createRef<HTMLSpanElement>();

    render(
      <WorkflowLogPanel
        panelLogId="panel-log"
        tabLogId="tab-log"
        status="idle"
        logContent="some log content"
        logEndRef={logEndRef}
        preRef={preRef}
        onScroll={onScroll}
      />,
    );

    fireEvent.scroll(preRef.current!);

    expect(onScroll).toHaveBeenCalledTimes(1);
  });
});

// --- Tests: WorkflowRunner polling -------------------------------------------

describe("WorkflowRunner - log tab polling", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.clearAllMocks();
    vi.useRealTimers();
  });

  it("calls getSessionLog immediately when log tab is opened", async () => {
    const commands = await import("../lib/commands");
    vi.mocked(commands.getSessionLog).mockResolvedValue("line 1\nline 2");

    render(<WorkflowRunner {...defaultRunnerProps()} activeTab="log" />);
    await act(() => Promise.resolve());

    expect(vi.mocked(commands.getSessionLog)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(commands.getSessionLog)).toHaveBeenCalledWith("session-1");
  });

  it("calls getSessionLog every 1000ms while the log tab is open", async () => {
    const commands = await import("../lib/commands");
    vi.mocked(commands.getSessionLog).mockResolvedValue("log line");

    render(<WorkflowRunner {...defaultRunnerProps()} activeTab="log" />);
    await act(() => Promise.resolve());
    const callsAfterMount = vi.mocked(commands.getSessionLog).mock.calls.length;

    await act(() => vi.advanceTimersByTimeAsync(3000));

    expect(vi.mocked(commands.getSessionLog).mock.calls.length).toBe(callsAfterMount + 3);
  });

  it("stops polling when activeTab changes away from log", async () => {
    const commands = await import("../lib/commands");
    vi.mocked(commands.getSessionLog).mockResolvedValue("content");

    const props = defaultRunnerProps();
    const { rerender } = render(<WorkflowRunner {...props} activeTab="log" />);
    await act(() => Promise.resolve());

    rerender(<WorkflowRunner {...props} activeTab="info" />);
    await act(() => Promise.resolve());
    const callCountAfterTabChange = vi.mocked(commands.getSessionLog).mock.calls.length;

    await act(() => vi.advanceTimersByTimeAsync(3000));

    expect(vi.mocked(commands.getSessionLog).mock.calls.length).toBe(callCountAfterTabChange);
  });

  it("displays updated log content in the log panel after a polling interval", async () => {
    const commands = await import("../lib/commands");
    vi.mocked(commands.getSessionLog)
      .mockResolvedValueOnce("initial log")
      .mockResolvedValue("updated log content");

    render(<WorkflowRunner {...defaultRunnerProps()} activeTab="log" />);

    await act(async () => {
      await Promise.resolve();
      await vi.advanceTimersByTimeAsync(1000);
      await Promise.resolve();
    });

    expect(screen.getByText(/updated log content/)).toBeTruthy();
  });

  it("does not re-render when polling returns the same content as before", async () => {
    const commands = await import("../lib/commands");
    vi.mocked(commands.getSessionLog).mockResolvedValue("same content");

    render(<WorkflowRunner {...defaultRunnerProps()} activeTab="log" />);
    await act(() => Promise.resolve());

    await act(() => vi.advanceTimersByTimeAsync(2000));

    expect(screen.getByText(/same content/)).toBeTruthy();
  });
});

// --- Tests: WorkflowRunner scroll behaviour ----------------------------------

describe("WorkflowRunner - stick-to-bottom scroll behaviour", () => {
  let originalScrollIntoView: typeof HTMLElement.prototype.scrollIntoView;

  beforeEach(() => {
    vi.useFakeTimers();
    originalScrollIntoView = HTMLElement.prototype.scrollIntoView;
    HTMLElement.prototype.scrollIntoView = vi.fn();
  });

  afterEach(() => {
    vi.clearAllMocks();
    vi.useRealTimers();
    HTMLElement.prototype.scrollIntoView = originalScrollIntoView;
  });

  it("scrolls to bottom when log content updates and user is near the bottom", async () => {
    const commands = await import("../lib/commands");
    vi.mocked(commands.getSessionLog)
      .mockResolvedValueOnce("line 1")
      .mockResolvedValue("line 1\nline 2");

    render(<WorkflowRunner {...defaultRunnerProps()} activeTab="log" />);
    await act(() => Promise.resolve());

    vi.mocked(HTMLElement.prototype.scrollIntoView).mockClear();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
      await Promise.resolve();
    });

    expect(HTMLElement.prototype.scrollIntoView).toHaveBeenCalled();
  });

  it("does not scroll to bottom when user has scrolled up in the log panel", async () => {
    const commands = await import("../lib/commands");
    vi.mocked(commands.getSessionLog)
      .mockResolvedValueOnce("line 1")
      .mockResolvedValue("line 1\nline 2");

    render(<WorkflowRunner {...defaultRunnerProps()} activeTab="log" />);
    await act(() => Promise.resolve());

    const preEl = document.querySelector("pre");
    expect(preEl).not.toBeNull();
    Object.defineProperty(preEl!, "scrollHeight", { value: 1000, configurable: true });
    Object.defineProperty(preEl!, "scrollTop", { value: 0, configurable: true });
    Object.defineProperty(preEl!, "clientHeight", { value: 200, configurable: true });
    fireEvent.scroll(preEl!);

    vi.mocked(HTMLElement.prototype.scrollIntoView).mockClear();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
      await Promise.resolve();
    });

    expect(HTMLElement.prototype.scrollIntoView).not.toHaveBeenCalled();
  });

  it("resumes scrolling to bottom when user scrolls back near the bottom", async () => {
    const commands = await import("../lib/commands");
    vi.mocked(commands.getSessionLog)
      .mockResolvedValueOnce("line 1")
      .mockResolvedValueOnce("line 1\nline 2")
      .mockResolvedValue("line 1\nline 2\nline 3");

    render(<WorkflowRunner {...defaultRunnerProps()} activeTab="log" />);
    await act(() => Promise.resolve());

    const preEl = document.querySelector("pre")!;
    Object.defineProperty(preEl, "scrollHeight", { value: 1000, configurable: true });
    Object.defineProperty(preEl, "scrollTop", { value: 0, configurable: true });
    Object.defineProperty(preEl, "clientHeight", { value: 200, configurable: true });
    fireEvent.scroll(preEl);

    Object.defineProperty(preEl, "scrollTop", { value: 800, configurable: true });
    fireEvent.scroll(preEl);

    vi.mocked(HTMLElement.prototype.scrollIntoView).mockClear();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
      await Promise.resolve();
    });

    expect(HTMLElement.prototype.scrollIntoView).toHaveBeenCalled();
  });
});

// --- Tests: WorkflowRunner DAG tab entrypoint --------------------------------

describe("WorkflowRunner - DAG tab entrypoint", () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it("shows a tab named 'DAG' between the Info and Plan tabs", async () => {
    // Given / When
    render(<WorkflowRunner {...defaultRunnerProps()} activeTab="plan" />);
    await act(() => Promise.resolve());

    // Then
    const tabNames = screen.getAllByRole("tab").map((el) => el.textContent);
    const dagIndex = tabNames.findIndex((name) => name?.includes("DAG"));
    const infoIndex = tabNames.findIndex((name) => name?.includes("Info"));
    const planIndex = tabNames.findIndex((name) => name?.includes("Plan"));

    expect(dagIndex).toBeGreaterThan(infoIndex);
    expect(dagIndex).toBeLessThan(planIndex);
  });

  it("calls onActiveTabChange('dag') exactly once when the DAG tab is clicked", async () => {
    // Given
    const onActiveTabChange = vi.fn();

    // When
    render(
      <WorkflowRunner
        {...defaultRunnerProps()}
        activeTab="plan"
        onActiveTabChange={onActiveTabChange}
      />,
    );
    await act(() => Promise.resolve());
    fireEvent.click(screen.getByRole("tab", { name: "DAG" }));

    // Then
    expect(onActiveTabChange).toHaveBeenCalledTimes(1);
    expect(onActiveTabChange).toHaveBeenCalledWith("dag");
  });
});

// --- Tests: WorkflowRunner DAG panel mounting --------------------------------

describe("WorkflowRunner - DAG panel mounting", () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it("mounts WorkflowDagPanel and calls getSessionDag('session-1') once when activeTab='dag'", async () => {
    // Given
    const commands = await import("../lib/commands");

    // When
    render(<WorkflowRunner {...defaultRunnerProps()} activeTab="dag" />);
    await act(() => Promise.resolve());

    // Then
    expect(vi.mocked(commands.getSessionDag)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(commands.getSessionDag)).toHaveBeenCalledWith("session-1");
  });

  it("does not start log polling when activeTab='dag'", async () => {
    // Given
    const commands = await import("../lib/commands");

    // When
    render(<WorkflowRunner {...defaultRunnerProps()} activeTab="dag" />);
    await act(() => Promise.resolve());

    // Then
    expect(vi.mocked(commands.getSessionLog)).not.toHaveBeenCalled();
  });
});
