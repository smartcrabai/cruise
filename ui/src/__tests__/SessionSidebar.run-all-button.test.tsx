import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SessionSidebar } from "../components/SessionSidebar";
import type { Session } from "../types";
import * as commands from "../lib/commands";

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn().mockResolvedValue("0.0.0"),
}));

vi.mock("../lib/updater", () => ({
  checkForUpdate: vi.fn().mockResolvedValue(null),
  downloadAndInstall: vi.fn(),
}));

vi.mock("../lib/commands", () => ({
  listSessions: vi.fn(),
  cleanSessions: vi.fn(),
  getUpdateReadiness: vi.fn().mockResolvedValue({ canAutoUpdate: true }),
}));

// --- Helpers ---

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

// --- Tests ---

describe("SessionSidebar: Run All button — active state and enablement", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listSessions).mockResolvedValue([]);
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  // --- Default (no runAllActive) ---

  it("is disabled when there are no Planned or Suspended sessions", async () => {
    // Given: only Completed sessions
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "s1", phase: "Completed" }),
    ]);

    render(
      <SessionSidebar
        selectedId={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onRunAll={vi.fn()}
      />,
    );

    await waitFor(() =>
      expect(commands.listSessions).toHaveBeenCalledOnce(),
    );

    // Then: Run All is disabled
    expect(screen.getByText("Run All")).toBeDisabled();
  });

  it("is enabled when there are Planned sessions", async () => {
    // Given
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "s1", phase: "Planned" }),
    ]);

    render(
      <SessionSidebar
        selectedId={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onRunAll={vi.fn()}
      />,
    );

    await waitFor(() =>
      expect(commands.listSessions).toHaveBeenCalledOnce(),
    );

    // Then
    expect(screen.getByText("Run All")).not.toBeDisabled();
  });

  it("is enabled when there are Suspended sessions", async () => {
    // Given
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "s1", phase: "Suspended" }),
    ]);

    render(
      <SessionSidebar
        selectedId={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onRunAll={vi.fn()}
      />,
    );

    await waitFor(() =>
      expect(commands.listSessions).toHaveBeenCalledOnce(),
    );

    // Then
    expect(screen.getByText("Run All")).not.toBeDisabled();
  });

  it("calls onRunAll when clicked", async () => {
    // Given
    const onRunAll = vi.fn();
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "s1", phase: "Planned" }),
    ]);

    render(
      <SessionSidebar
        selectedId={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onRunAll={onRunAll}
      />,
    );

    await waitFor(() =>
      expect(commands.listSessions).toHaveBeenCalledOnce(),
    );

    // When
    await userEvent.click(screen.getByText("Run All"));

    // Then
    expect(onRunAll).toHaveBeenCalledOnce();
  });

  // --- runAllActive = true (execution in progress) ---

  it("is enabled when runAllActive is true, even with no runnable sessions", async () => {
    // Given: all sessions are completed, but Run All is active
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "s1", phase: "Completed" }),
    ]);

    render(
      <SessionSidebar
        selectedId={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onRunAll={vi.fn()}
        runAllActive
      />,
    );

    await waitFor(() =>
      expect(commands.listSessions).toHaveBeenCalledOnce(),
    );

    // Then: Run All is enabled despite no runnable sessions
    expect(screen.getByText("Run All")).not.toBeDisabled();
  });

  it("has active styling when runAllActive is true", async () => {
    // Given
    vi.mocked(commands.listSessions).mockResolvedValue([]);

    render(
      <SessionSidebar
        selectedId={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onRunAll={vi.fn()}
        runAllActive
      />,
    );

    await waitFor(() =>
      expect(commands.listSessions).toHaveBeenCalledOnce(),
    );

    // Then: button has active-specific class (e.g. bg-blue-600)
    const button = screen.getByText("Run All");
    expect(button.className).toContain("bg-blue-600");
  });

  // --- runAllActive = false (default) ---

  it("does not have active styling when runAllActive is false or absent", async () => {
    // Given
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "s1", phase: "Planned" }),
    ]);

    render(
      <SessionSidebar
        selectedId={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onRunAll={vi.fn()}
      />,
    );

    await waitFor(() =>
      expect(commands.listSessions).toHaveBeenCalledOnce(),
    );

    // Then: button does NOT have active styling
    const button = screen.getByText("Run All");
    expect(button.className).not.toContain("bg-blue-600");
  });

  it("is disabled when runAllActive is false and no runnable sessions exist", async () => {
    // Given: no runnable sessions, runAllActive not set
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "s1", phase: "Running" }),
    ]);

    render(
      <SessionSidebar
        selectedId={null}
        onSelect={vi.fn()}
        onNewSession={vi.fn()}
        onRunAll={vi.fn()}
        runAllActive={false}
      />,
    );

    await waitFor(() =>
      expect(commands.listSessions).toHaveBeenCalledOnce(),
    );

    // Then
    expect(screen.getByText("Run All")).toBeDisabled();
  });
});
