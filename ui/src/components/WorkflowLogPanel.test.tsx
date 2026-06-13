import { createRef } from "react";
import { render, screen, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { WorkflowLogPanel } from "./WorkflowLogPanel";

afterEach(() => cleanup());

const baseProps = {
  panelLogId: "panel-log-1",
  tabLogId: "tab-log-1",
  status: "idle" as const,
  logContent: "",
  logEndRef: createRef<HTMLSpanElement | null>(),
  preRef: createRef<HTMLPreElement | null>(),
  onScroll: vi.fn(),
};

describe("WorkflowLogPanel", () => {
  describe("role / accessibility", () => {
    it("element with role='tabpanel' exists", () => {
      // Given / When
      render(<WorkflowLogPanel {...baseProps} />);
      // Then
      expect(screen.getByRole("tabpanel")).toBeInTheDocument();
    });

    it("panelLogId is set as id", () => {
      // Given / When
      render(<WorkflowLogPanel {...baseProps} />);
      // Then
      expect(screen.getByRole("tabpanel")).toHaveAttribute("id", "panel-log-1");
    });
  });

  describe("Message when log is empty", () => {
    it("'Generate a plan or run the session to see logs here.' is shown when status='idle' and logContent is empty", () => {
      // Given / When
      render(<WorkflowLogPanel {...baseProps} status="idle" logContent="" />);
      // Then
      expect(screen.getByText("Generate a plan or run the session to see logs here.")).toBeInTheDocument();
    });

    it("'No log entries yet.' is shown when status='running' and logContent is empty", () => {
      // Given / When
      render(<WorkflowLogPanel {...baseProps} status="running" logContent="" />);
      // Then
      expect(screen.getByText("No log entries yet.")).toBeInTheDocument();
    });

    it("'No log entries yet.' is shown when status='completed' and logContent is empty", () => {
      // Given / When
      render(<WorkflowLogPanel {...baseProps} status="completed" logContent="" />);
      // Then
      expect(screen.getByText("No log entries yet.")).toBeInTheDocument();
    });

    it("'No log entries yet.' is shown when status='failed' and logContent is empty", () => {
      // Given / When
      render(<WorkflowLogPanel {...baseProps} status="failed" logContent="" />);
      // Then
      expect(screen.getByText("No log entries yet.")).toBeInTheDocument();
    });
  });

  describe("Log content display", () => {
    it("content is shown when logContent is present", () => {
      // Given / When
      render(
        <WorkflowLogPanel
          {...baseProps}
          logContent="Step 1 started\nStep 1 completed"
        />
      );
      // Then
      expect(screen.getByText(/Step 1 started/)).toBeInTheDocument();
    });

    it("empty message is not shown when logContent is present", () => {
      // Given / When
      render(
        <WorkflowLogPanel
          {...baseProps}
          status="idle"
          logContent="some log output"
        />
      );
      // Then
      expect(screen.queryByText("Generate a plan or run the session to see logs here.")).not.toBeInTheDocument();
    });
  });
});
