import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup, act } from "@testing-library/react";
import type { MutableRefObject } from "react";
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

const defaultProps = {
  selectedId: null as string | null,
  onSelect: vi.fn(),
  onNewSession: vi.fn(),
  onRunAll: vi.fn(),
};

// --- Tests ---

describe("SessionSidebar: onOptimisticRemoveRef", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listSessions).mockResolvedValue([]);
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("wires a remove function to the ref after mount", async () => {
    // Given
    const optimisticRemoveRef: MutableRefObject<((id: string) => void) | null> = {
      current: null,
    };

    // When
    render(
      <SessionSidebar
        {...defaultProps}
        onOptimisticRemoveRef={optimisticRemoveRef}
      />
    );

    // Then: ref is wired once the component mounts and loads
    await waitFor(() => {
      expect(optimisticRemoveRef.current).toBeTypeOf("function");
    });
  });

  it("removes the matching session from the rendered list", async () => {
    // Given: sidebar with two sessions
    const optimisticRemoveRef: MutableRefObject<((id: string) => void) | null> = {
      current: null,
    };
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "session-1", input: "first task" }),
      makeSession({ id: "session-2", input: "second task" }),
    ]);

    render(
      <SessionSidebar
        {...defaultProps}
        onOptimisticRemoveRef={optimisticRemoveRef}
      />
    );
    await waitFor(() => screen.getByText("first task"));
    await waitFor(() => expect(optimisticRemoveRef.current).toBeTypeOf("function"));

    // When: optimistic remove is called for session-1
    act(() => {
      optimisticRemoveRef.current!("session-1");
    });

    // Then: session-1 disappears, session-2 remains
    expect(screen.queryByText("first task")).toBeNull();
    expect(screen.getByText("second task")).toBeInTheDocument();
  });

  it("leaves the list unchanged when called with an unknown ID", async () => {
    // Given: sidebar with one session
    const optimisticRemoveRef: MutableRefObject<((id: string) => void) | null> = {
      current: null,
    };
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "session-1", input: "only task" }),
    ]);

    render(
      <SessionSidebar
        {...defaultProps}
        onOptimisticRemoveRef={optimisticRemoveRef}
      />
    );
    await waitFor(() => screen.getByText("only task"));
    await waitFor(() => expect(optimisticRemoveRef.current).toBeTypeOf("function"));

    // When: called with an ID not in the list
    act(() => {
      optimisticRemoveRef.current!("nonexistent-session");
    });

    // Then: the existing session is still displayed
    expect(screen.getByText("only task")).toBeInTheDocument();
  });

  it("sets the ref to null when the component unmounts", async () => {
    // Given
    const optimisticRemoveRef: MutableRefObject<((id: string) => void) | null> = {
      current: null,
    };

    const { unmount } = render(
      <SessionSidebar
        {...defaultProps}
        onOptimisticRemoveRef={optimisticRemoveRef}
      />
    );
    await waitFor(() => expect(optimisticRemoveRef.current).toBeTypeOf("function"));

    // When: component unmounts
    unmount();

    // Then: ref is cleared to prevent stale calls
    expect(optimisticRemoveRef.current).toBeNull();
  });

  it("renders normally when onOptimisticRemoveRef is omitted", async () => {
    // When: render without the optional prop
    render(<SessionSidebar {...defaultProps} />);

    // Then: no error, listSessions is still called
    await waitFor(() => {
      expect(commands.listSessions).toHaveBeenCalledOnce();
    });
  });
});
