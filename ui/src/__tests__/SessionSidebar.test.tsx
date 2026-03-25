import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, act, waitFor, cleanup } from "@testing-library/react";
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

describe("SessionSidebar", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(commands.listSessions).mockResolvedValue([]);
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("calls listSessions after mount", async () => {
    // When
    render(<SessionSidebar {...defaultProps} />);

    // Then
    await waitFor(() => {
      expect(commands.listSessions).toHaveBeenCalledOnce();
    });
  });

  it("shows Loading... while loading", async () => {
    // Given: first listSessions is pending
    let resolve!: (v: Session[]) => void;
    vi.mocked(commands.listSessions).mockReturnValueOnce(
      new Promise<Session[]>((r) => {
        resolve = r;
      }),
    );

    // When
    render(<SessionSidebar {...defaultProps} />);

    // Then: loading indicator is shown
    expect(screen.getByText("Loading...")).toBeTruthy();

    // Cleanup
    await act(async () => {
      resolve([]);
    });
  });

  it("shows session list when listSessions succeeds", async () => {
    // Given
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "abc123", input: "hello world" }),
    ]);

    // When
    render(<SessionSidebar {...defaultProps} />);

    // Then
    await waitFor(() => {
      expect(screen.getByText("hello world")).toBeTruthy();
    });
  });

  it("shows error when listSessions fails", async () => {
    // Given
    vi.mocked(commands.listSessions).mockRejectedValue(
      new Error("network error"),
    );

    // When
    render(<SessionSidebar {...defaultProps} />);

    // Then
    await waitFor(() => {
      expect(screen.getByText("Error: Error: network error")).toBeTruthy();
    });
  });

  it("refresh via onRefreshRef does not show loading (silent mode)", async () => {
    // Given: slow refresh after initial load completes
    vi.mocked(commands.listSessions).mockResolvedValueOnce([]);
    let resolveRefresh!: (v: Session[]) => void;
    vi.mocked(commands.listSessions).mockReturnValueOnce(
      new Promise<Session[]>((r) => {
        resolveRefresh = r;
      }),
    );

    const refreshRef = { current: null as (() => void) | null };
    render(
      <SessionSidebar {...defaultProps} onRefreshRef={refreshRef} />,
    );

    // Wait for initial load to complete
    await waitFor(() =>
      expect(commands.listSessions).toHaveBeenCalledTimes(1),
    );

    // When: refresh via ref
    act(() => {
      refreshRef.current?.();
    });

    // Then: no loading indicator (silent mode)
    expect(screen.queryByText("Loading...")).toBeNull();

    // Cleanup
    await act(async () => {
      resolveRefresh([]);
    });
  });

  it("failure via onRefreshRef does not show error (silent mode)", async () => {
    // Given: initial load succeeds, subsequent refresh fails
    vi.mocked(commands.listSessions).mockResolvedValueOnce([]);
    vi.mocked(commands.listSessions).mockRejectedValueOnce(
      new Error("poll error"),
    );

    const refreshRef = { current: null as (() => void) | null };
    render(
      <SessionSidebar {...defaultProps} onRefreshRef={refreshRef} />,
    );

    await waitFor(() =>
      expect(commands.listSessions).toHaveBeenCalledTimes(1),
    );

    // When
    await act(async () => {
      refreshRef.current?.();
    });

    // Then: no error shown
    expect(screen.queryByText(/Error:/)).toBeNull();
  });

  it("success via onRefreshRef clears existing error (silent mode)", async () => {
    // Given: initial load fails -> error is shown
    vi.mocked(commands.listSessions).mockRejectedValueOnce(
      new Error("initial error"),
    );
    vi.mocked(commands.listSessions).mockResolvedValueOnce([]);

    const refreshRef = { current: null as (() => void) | null };
    render(
      <SessionSidebar {...defaultProps} onRefreshRef={refreshRef} />,
    );

    // Verify initial error
    await waitFor(() => {
      expect(screen.queryByText(/Error:/)).not.toBeNull();
    });

    // When: silent refresh succeeds
    await act(async () => {
      refreshRef.current?.();
    });

    // Then: error is cleared
    await waitFor(() => {
      expect(screen.queryByText(/Error:/)).toBeNull();
    });
  });

  it("calls listSessions every 3 seconds (polling)", async () => {
    // Given
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    vi.mocked(commands.listSessions).mockResolvedValue([]);

    render(<SessionSidebar {...defaultProps} />);
    // Resolve the initial load Promise
    await act(async () => {
      await Promise.resolve();
    });

    const callsAfterMount = vi.mocked(commands.listSessions).mock.calls.length;

    // When: 3 seconds pass
    await act(async () => {
      vi.advanceTimersByTime(3000);
      await Promise.resolve();
    });

    // Then: listSessions is called additionally
    expect(vi.mocked(commands.listSessions).mock.calls.length).toBeGreaterThan(
      callsAfterMount,
    );
  });

  it("does not show loading during polling (silent mode)", async () => {
    // Given
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    // Initial load: completes immediately
    vi.mocked(commands.listSessions).mockResolvedValueOnce([]);
    // Polling call: set to pending state
    let resolvePolling!: (v: Session[]) => void;
    vi.mocked(commands.listSessions).mockReturnValueOnce(
      new Promise<Session[]>((r) => {
        resolvePolling = r;
      }),
    );

    render(<SessionSidebar {...defaultProps} />);
    // Initial load complete
    await act(async () => {
      await Promise.resolve();
    });

    // When: polling fires
    act(() => {
      vi.advanceTimersByTime(3000);
    });

    // Then: no loading indicator even while pending (silent mode)
    expect(screen.queryByText("Loading...")).toBeNull();

    // Cleanup
    await act(async () => {
      resolvePolling([]);
    });
  });

  it("skips polling when visibilityState is hidden", async () => {
    // Given
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
    vi.mocked(commands.listSessions).mockResolvedValue([]);

    render(<SessionSidebar {...defaultProps} />);
    await act(async () => {
      await Promise.resolve();
    });

    const callsAfterMount = vi.mocked(commands.listSessions).mock.calls.length;

    // When: 9 seconds pass (3 polling intervals)
    await act(async () => {
      vi.advanceTimersByTime(9000);
      await Promise.resolve();
    });

    // Then: no additional calls while window is hidden
    expect(vi.mocked(commands.listSessions).mock.calls.length).toBe(
      callsAfterMount,
    );
  });

  it("stops polling after unmount", async () => {
    // Given
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    vi.mocked(commands.listSessions).mockResolvedValue([]);

    const { unmount } = render(<SessionSidebar {...defaultProps} />);
    await act(async () => {
      await Promise.resolve();
    });

    // Confirm polling works before unmounting
    await act(async () => {
      vi.advanceTimersByTime(3000);
      await Promise.resolve();
    });
    const callsBeforeUnmount =
      vi.mocked(commands.listSessions).mock.calls.length;

    // When: unmount
    unmount();

    // Then: listSessions is not called even after more time passes
    await act(async () => {
      vi.advanceTimersByTime(9000);
      await Promise.resolve();
    });
    expect(vi.mocked(commands.listSessions).mock.calls.length).toBe(
      callsBeforeUnmount,
    );
  });

  // ─── onSelectedSessionUpdated callback ─────────────────────────────────────

  it("calls onSelectedSessionUpdated when load returns a session matching selectedId", async () => {
    // Given: selectedId matches a session in the initial load result
    const session = makeSession({ id: "session-1", phase: "Planned" });
    vi.mocked(commands.listSessions).mockResolvedValue([session]);

    const onSelectedSessionUpdated = vi.fn();

    // When
    render(
      <SessionSidebar
        {...defaultProps}
        selectedId="session-1"
        onSelectedSessionUpdated={onSelectedSessionUpdated}
      />,
    );

    // Then: callback is called with the latest DTO for the selected session
    await waitFor(() => {
      expect(onSelectedSessionUpdated).toHaveBeenCalledWith(
        expect.objectContaining({ id: "session-1", phase: "Planned" }),
      );
    });
  });

  it("calls onSelectedSessionUpdated with updated session after a silent refresh", async () => {
    // Given: initial load returns one state, then a refresh returns an updated state
    const initial = makeSession({ id: "session-1", phase: "Planned" });
    vi.mocked(commands.listSessions).mockResolvedValueOnce([initial]);
    const updated = makeSession({ id: "session-1", phase: "Running" });
    vi.mocked(commands.listSessions).mockResolvedValueOnce([updated]);

    const onSelectedSessionUpdated = vi.fn();
    const refreshRef = { current: null as (() => void) | null };

    render(
      <SessionSidebar
        {...defaultProps}
        selectedId="session-1"
        onSelectedSessionUpdated={onSelectedSessionUpdated}
        onRefreshRef={refreshRef}
      />,
    );

    // Wait for the initial load to complete
    await waitFor(() =>
      expect(commands.listSessions).toHaveBeenCalledTimes(1),
    );

    // When: silent refresh fires (polling or visibility change)
    await act(async () => {
      refreshRef.current?.();
    });

    // Then: parent receives the updated session
    await waitFor(() => {
      expect(onSelectedSessionUpdated).toHaveBeenLastCalledWith(
        expect.objectContaining({ id: "session-1", phase: "Running" }),
      );
    });
  });

  it("does not call onSelectedSessionUpdated when selectedId is null", async () => {
    // Given: no session is selected
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "session-1" }),
    ]);
    const onSelectedSessionUpdated = vi.fn();

    // When
    render(
      <SessionSidebar
        {...defaultProps}
        selectedId={null}
        onSelectedSessionUpdated={onSelectedSessionUpdated}
      />,
    );

    await waitFor(() =>
      expect(commands.listSessions).toHaveBeenCalledTimes(1),
    );

    // Then: callback is never called (nothing is selected)
    expect(onSelectedSessionUpdated).not.toHaveBeenCalled();
  });

  it("does not call onSelectedSessionUpdated when selectedId is not in the result", async () => {
    // Given: the selected session is absent from the returned list
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "other-session" }),
    ]);
    const onSelectedSessionUpdated = vi.fn();

    // When
    render(
      <SessionSidebar
        {...defaultProps}
        selectedId="session-1"
        onSelectedSessionUpdated={onSelectedSessionUpdated}
      />,
    );

    await waitFor(() =>
      expect(commands.listSessions).toHaveBeenCalledTimes(1),
    );

    // Then: callback is not called (session no longer exists in the list)
    expect(onSelectedSessionUpdated).not.toHaveBeenCalled();
  });

  // ─── Header buttons ────────────────────────────────────────────────────────

  it("renders Clean, Run All, and + New buttons but no Refresh button in the header", async () => {
    // Given
    vi.mocked(commands.listSessions).mockResolvedValue([]);

    // When
    render(<SessionSidebar {...defaultProps} />);
    await waitFor(() =>
      expect(commands.listSessions).toHaveBeenCalledTimes(1),
    );

    // Then
    expect(screen.queryByTitle("Refresh")).toBeNull();
    expect(screen.getByText("Clean")).toBeTruthy();
    expect(screen.getByText("Run All")).toBeTruthy();
    expect(screen.getByText("+ New")).toBeTruthy();
  });

  it("calls listSessions immediately when visibilitychange makes document visible", async () => {
    // Given
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    vi.mocked(commands.listSessions).mockResolvedValue([]);

    render(<SessionSidebar {...defaultProps} />);
    await act(async () => {
      await Promise.resolve();
    });

    const callsAfterMount = vi.mocked(commands.listSessions).mock.calls.length;

    // Set window to visible state
    vi.spyOn(document, "visibilityState", "get").mockReturnValue("visible");

    // When: fire visibilitychange event
    await act(async () => {
      document.dispatchEvent(new Event("visibilitychange"));
      await Promise.resolve();
    });

    // Then: listSessions is called immediately without waiting for interval
    expect(vi.mocked(commands.listSessions).mock.calls.length).toBeGreaterThan(
      callsAfterMount,
    );
  });

  // ─── PhaseBadge planAvailable indicator ────────────────────────────────────

  it("shows blue dot indicator for 'Awaiting Approval' session when planAvailable is true", async () => {
    // Given: a session that is awaiting approval with a plan already generated
    vi.mocked(commands.listSessions).mockResolvedValue([
      makeSession({ id: "session-1", phase: "Awaiting Approval", planAvailable: true }),
    ]);

    // When
    render(<SessionSidebar {...defaultProps} />);

    // Then: the blue dot indicator is shown for that session
    await waitFor(() => {
      expect(screen.getByLabelText("plan ready for approval")).toBeTruthy();
    });
  });

});
