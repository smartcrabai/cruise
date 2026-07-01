import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, cleanup, act } from "@testing-library/react";
import type { DagDto } from "../types";

// WorkflowDagPanel keeps a module-level `mermaidInitialized` flag, so each test
// needs a fresh module instance (vi.resetModules) to observe initialize() calls
// in isolation -- otherwise only the first test in this file would see the call.

let mockGetSessionDag: ReturnType<typeof vi.fn>;
let mockMermaidInitialize: ReturnType<typeof vi.fn>;
let mockMermaidRender: ReturnType<typeof vi.fn>;
let WorkflowDagPanel: typeof import("./WorkflowDagPanel").WorkflowDagPanel;

beforeEach(async () => {
  vi.resetModules();
  mockGetSessionDag = vi.fn();
  mockMermaidInitialize = vi.fn();
  mockMermaidRender = vi.fn();
  vi.doMock("../lib/commands", () => ({
    getSessionDag: mockGetSessionDag,
  }));
  vi.doMock("mermaid", () => ({
    default: {
      initialize: mockMermaidInitialize,
      render: mockMermaidRender,
    },
  }));
  const mod = await import("./WorkflowDagPanel");
  WorkflowDagPanel = mod.WorkflowDagPanel;
});

afterEach(() => cleanup());

const baseProps = {
  panelId: "panel-dag-1",
  tabId: "tab-dag-1",
};

function makeDag(overrides: Partial<DagDto> = {}): DagDto {
  return {
    startStep: "build",
    currentStep: "test",
    steps: [
      { name: "build", kind: "command", isTerminal: false },
      { name: "test", kind: "command", isTerminal: true },
    ],
    edges: [
      { from: "build", to: "test", reason: "ifFileChanged", selector: "src/**" },
      { from: "test", to: null, reason: "sequential", selector: null },
    ],
    ...overrides,
  };
}

describe("WorkflowDagPanel", () => {
  describe("role / accessibility", () => {
    it("element with role='tabpanel' exists", () => {
      // Given / When
      mockGetSessionDag.mockReturnValue(new Promise(() => {}));
      render(<WorkflowDagPanel {...baseProps} sessionId="session-1" />);
      // Then
      expect(screen.getByRole("tabpanel")).toBeInTheDocument();
    });

    it("panelId is set as id", () => {
      // Given / When
      mockGetSessionDag.mockReturnValue(new Promise(() => {}));
      render(<WorkflowDagPanel {...baseProps} sessionId="session-1" />);
      // Then
      expect(screen.getByRole("tabpanel")).toHaveAttribute("id", "panel-dag-1");
    });
  });

  describe("loading state", () => {
    it("'Loading DAG…' is shown before getSessionDag resolves", () => {
      // Given: getSessionDag never resolves within this test
      mockGetSessionDag.mockReturnValue(new Promise(() => {}));
      // When
      render(<WorkflowDagPanel {...baseProps} sessionId="session-1" />);
      // Then
      expect(screen.getByText("Loading DAG…")).toBeInTheDocument();
    });
  });

  describe("success state", () => {
    it("calls getSessionDag with the given sessionId", async () => {
      // Given
      mockGetSessionDag.mockResolvedValue(makeDag());
      mockMermaidRender.mockResolvedValue({ svg: "<svg></svg>" });
      // When
      render(<WorkflowDagPanel {...baseProps} sessionId="session-1" />);
      await waitFor(() => expect(mockMermaidRender).toHaveBeenCalled());
      // Then
      expect(mockGetSessionDag).toHaveBeenCalledTimes(1);
      expect(mockGetSessionDag).toHaveBeenCalledWith("session-1");
    });

    it("calls mermaid.initialize once with startOnLoad=false", async () => {
      // Given
      mockGetSessionDag.mockResolvedValue(makeDag());
      mockMermaidRender.mockResolvedValue({ svg: "<svg></svg>" });
      // When
      render(<WorkflowDagPanel {...baseProps} sessionId="session-1" />);
      await waitFor(() => expect(mockMermaidRender).toHaveBeenCalled());
      // Then
      expect(mockMermaidInitialize).toHaveBeenCalledTimes(1);
      expect(mockMermaidInitialize).toHaveBeenCalledWith({ startOnLoad: false, theme: "default" });
    });

    it("passes a Mermaid source with graph header, step labels, and terminal END node", async () => {
      // Given
      mockGetSessionDag.mockResolvedValue(makeDag());
      mockMermaidRender.mockResolvedValue({ svg: "<svg></svg>" });
      // When
      render(<WorkflowDagPanel {...baseProps} sessionId="session-1" />);
      await waitFor(() => expect(mockMermaidRender).toHaveBeenCalled());
      // Then
      const [, source] = mockMermaidRender.mock.calls[0];
      expect(source).toContain("graph TD");
      expect(source).toContain('s0_build["build"]');
      expect(source).toContain('s1_test["test"]');
      expect(source).toContain("end_terminal[/END/]");
    });

    it("passes a Mermaid source with an edge label derived from the edge reason/selector", async () => {
      // Given
      mockGetSessionDag.mockResolvedValue(makeDag());
      mockMermaidRender.mockResolvedValue({ svg: "<svg></svg>" });
      // When
      render(<WorkflowDagPanel {...baseProps} sessionId="session-1" />);
      await waitFor(() => expect(mockMermaidRender).toHaveBeenCalled());
      // Then
      const [, source] = mockMermaidRender.mock.calls[0];
      expect(source).toContain('s0_build -->|"if-file-changed: src/**"| s1_test');
      expect(source).toContain("s1_test --> end_terminal");
    });

    it("passes a Mermaid source with style lines for the start and current steps", async () => {
      // Given: startStep="build", currentStep="test"
      mockGetSessionDag.mockResolvedValue(makeDag());
      mockMermaidRender.mockResolvedValue({ svg: "<svg></svg>" });
      // When
      render(<WorkflowDagPanel {...baseProps} sessionId="session-1" />);
      await waitFor(() => expect(mockMermaidRender).toHaveBeenCalled());
      // Then
      const [, source] = mockMermaidRender.mock.calls[0];
      expect(source).toContain("style s0_build fill:#10b981,color:#fff,stroke:#059669,stroke-width:2px");
      expect(source).toContain("style s1_test fill:#3b82f6,color:#fff,stroke:#2563eb,stroke-width:2px");
    });

    it("injects the rendered SVG returned by mermaid.render into the panel", async () => {
      // Given
      mockGetSessionDag.mockResolvedValue(makeDag());
      mockMermaidRender.mockResolvedValue({ svg: '<svg data-testid="mock-svg"></svg>' });
      // When
      const { container } = render(<WorkflowDagPanel {...baseProps} sessionId="session-1" />);
      // Then
      await waitFor(() => {
        expect(container.querySelector('[data-testid="mock-svg"]')).not.toBeNull();
      });
    });
  });

  describe("empty state", () => {
    it("'No DAG available.' is shown when steps is empty", async () => {
      // Given
      mockGetSessionDag.mockResolvedValue(makeDag({ steps: [], edges: [], currentStep: null }));
      // When
      render(<WorkflowDagPanel {...baseProps} sessionId="session-1" />);
      // Then
      await waitFor(() => expect(screen.getByText("No DAG available.")).toBeInTheDocument());
    });

    it("does not call mermaid.render when steps is empty", async () => {
      // Given
      mockGetSessionDag.mockResolvedValue(makeDag({ steps: [], edges: [], currentStep: null }));
      // When
      render(<WorkflowDagPanel {...baseProps} sessionId="session-1" />);
      await waitFor(() => expect(screen.getByText("No DAG available.")).toBeInTheDocument());
      // Then
      expect(mockMermaidRender).not.toHaveBeenCalled();
      expect(mockMermaidInitialize).not.toHaveBeenCalled();
    });
  });

  describe("error state", () => {
    it("shows 'Failed to render DAG:' with the error text when getSessionDag rejects", async () => {
      // Given
      mockGetSessionDag.mockRejectedValue(new Error("network failure"));
      // When
      render(<WorkflowDagPanel {...baseProps} sessionId="session-1" />);
      // Then
      await waitFor(() => {
        expect(screen.getByText(/Failed to render DAG:/)).toBeInTheDocument();
      });
      expect(screen.getByText(/network failure/)).toBeInTheDocument();
    });

    it("shows 'Failed to render DAG:' with the error text when mermaid.render rejects", async () => {
      // Given
      mockGetSessionDag.mockResolvedValue(makeDag());
      mockMermaidRender.mockRejectedValue(new Error("render failure"));
      // When
      render(<WorkflowDagPanel {...baseProps} sessionId="session-1" />);
      // Then
      await waitFor(() => {
        expect(screen.getByText(/Failed to render DAG:/)).toBeInTheDocument();
      });
      expect(screen.getByText(/render failure/)).toBeInTheDocument();
    });
  });

  describe("session switch behavior", () => {
    it("keeps the newer session's SVG when an older session's response resolves later", async () => {
      // Given: session-a's request stays pending; session-b's resolves immediately
      let resolveSessionA: ((dag: DagDto) => void) | undefined;
      const pendingSessionA = new Promise<DagDto>((resolve) => {
        resolveSessionA = resolve;
      });
      mockGetSessionDag.mockImplementation((sessionId: string) =>
        sessionId === "session-a"
          ? pendingSessionA
          : Promise.resolve(
              makeDag({
                steps: [{ name: "from-b", kind: "command", isTerminal: true }],
                startStep: "from-b",
                currentStep: null,
                edges: [],
              }),
            ),
      );
      mockMermaidRender.mockImplementation((_id: string, source: string) =>
        Promise.resolve({ svg: `<div class="rendered-source">${source}</div>` }),
      );

      // When: mount with session-a, then switch to session-b before session-a resolves
      const { rerender, container } = render(
        <WorkflowDagPanel {...baseProps} sessionId="session-a" />,
      );
      rerender(<WorkflowDagPanel {...baseProps} sessionId="session-b" />);

      await waitFor(() => {
        expect(container.querySelector(".rendered-source")).not.toBeNull();
      });
      expect(container.querySelector(".rendered-source")!.textContent).toContain("from-b");

      // When: session-a's stale response finally arrives
      await act(async () => {
        resolveSessionA!(
          makeDag({
            steps: [{ name: "from-a", kind: "command", isTerminal: true }],
            startStep: "from-a",
            currentStep: null,
            edges: [],
          }),
        );
        await Promise.resolve();
        await Promise.resolve();
      });

      // Then: the panel still shows session-b's SVG, not session-a's stale one
      expect(container.querySelector(".rendered-source")!.textContent).toContain("from-b");
      expect(container.querySelector(".rendered-source")!.textContent).not.toContain("from-a");
    });
  });
});
