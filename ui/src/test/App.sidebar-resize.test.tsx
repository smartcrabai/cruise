import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup, act, fireEvent } from "@testing-library/react";
import App from "../App";
import type { Session } from "../types";
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
  getAppConfig: vi.fn().mockResolvedValue({ runAllParallelism: 1 }),
  updateAppConfig: vi.fn(),
}));

vi.mock("../lib/updater", () => ({
  checkForUpdate: vi.fn().mockResolvedValue(null),
  downloadAndInstall: vi.fn(),
}));

vi.mock("../lib/desktopNotifications", () => ({
  notifyDesktop: vi.fn(),
}));

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

async function dragSidebar(delta: number) {
  const handle = screen.getByRole("separator", { name: /resize sidebar/i });
  await act(async () => {
    fireEvent.mouseDown(handle, { clientX: 0, clientY: 0 });
    fireEvent.mouseMove(document, { clientX: delta, clientY: 0 });
    fireEvent.mouseUp(document, { clientX: delta, clientY: 0 });
  });
}

describe("App: Sidebar resize", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    vi.mocked(commands.listSessions).mockResolvedValue([makeSession()]);
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

  it("resize handle is present with correct ARIA attributes", async () => {
    render(<App />);

    await waitFor(() => {
      const handle = screen.getByRole("separator", { name: /resize sidebar/i });
      expect(handle).toBeInTheDocument();
    });
  });

  it("resize handle has cursor-col-resize on hover", async () => {
    render(<App />);

    await waitFor(() => {
      const handle = screen.getByRole("separator", { name: /resize sidebar/i });
      expect(handle).toHaveClass("cursor-col-resize");
    });
  });

  it("sidebar has initial width from localStorage", async () => {
    localStorage.setItem("cruise-sidebar-width", "350");

    render(<App />);

    await waitFor(() => {
      const aside = screen.getByRole("complementary");
      expect(aside).toHaveStyle({ width: "350px" });
    });
  });

  it("sidebar has default width when localStorage is empty", async () => {
    render(<App />);

    await waitFor(() => {
      const aside = screen.getByRole("complementary");
      expect(aside).toHaveStyle({ width: "288px" });
    });
  });

  it("dragging left reduces sidebar width", async () => {
    render(<App />);

    await waitFor(() => {
      expect(screen.getByRole("complementary")).toBeInTheDocument();
    });

    const aside = screen.getByRole("complementary");
    expect(aside).toHaveStyle({ width: "288px" });

    await dragSidebar(-50);

    const newWidth = parseInt(aside.style.width, 10);
    expect(newWidth).toBeLessThan(288);
    expect(newWidth).toBeGreaterThanOrEqual(180);
  });

  it("sidebar width is clamped to minimum during drag", async () => {
    render(<App />);

    await waitFor(() => {
      expect(screen.getByRole("complementary")).toBeInTheDocument();
    });

    await dragSidebar(-500);

    const aside = screen.getByRole("complementary");
    expect(aside).toHaveStyle({ width: "180px" });
  });

  it("sidebar width is clamped to maximum during drag", async () => {
    render(<App />);

    await waitFor(() => {
      expect(screen.getByRole("complementary")).toBeInTheDocument();
    });

    await dragSidebar(500);

    const aside = screen.getByRole("complementary");
    expect(aside).toHaveStyle({ width: "480px" });
  });

  it("width persists in localStorage after drag", async () => {
    render(<App />);

    await waitFor(() => {
      expect(screen.getByRole("complementary")).toBeInTheDocument();
    });

    await dragSidebar(100);

    const stored = localStorage.getItem("cruise-sidebar-width");
    expect(stored).not.toBeNull();
    expect(parseInt(stored!, 10)).toBeGreaterThan(288);
  });

  it("user-select is disabled during drag", async () => {
    render(<App />);

    await waitFor(() => {
      expect(screen.getByRole("complementary")).toBeInTheDocument();
    });

    const handle = screen.getByRole("separator", { name: /resize sidebar/i });
    await act(async () => {
      fireEvent.mouseDown(handle, { clientX: 0, clientY: 0 });
    });

    expect(document.body.style.userSelect).toBe("none");
    expect(document.body.style.cursor).toBe("col-resize");

    await act(async () => {
      fireEvent.mouseUp(document, { clientX: 0, clientY: 0 });
    });
  });

  it("main content min-width-0 prevents flex shrinkage issues", async () => {
    render(<App />);

    await waitFor(() => {
      const main = screen.getByRole("main");
      expect(main).toHaveClass("min-w-0");
    });
  });
});
