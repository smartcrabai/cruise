import { render, screen, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { Session } from "../types";
import { WorkflowInfoPanel } from "./WorkflowInfoPanel";

afterEach(() => cleanup());

function makeSession(overrides: Partial<Session> = {}): Session {
  return {
    id: "session-1",
    phase: "Completed",
    configSource: "default.yaml",
    baseDir: "/home/user/project",
    input: "test task",
    createdAt: "2026-01-01T00:00:00Z",
    workspaceMode: "Worktree",
    skippedSteps: [],
    ...overrides,
  };
}

const panelProps = {
  panelInfoId: "panel-info-1",
  tabInfoId: "tab-info-1",
};

describe("WorkflowInfoPanel", () => {
  describe("role / accessibility", () => {
    it("element with role='tabpanel' exists", () => {
      // Given / When
      render(<WorkflowInfoPanel session={makeSession()} {...panelProps} />);
      // Then
      expect(screen.getByRole("tabpanel")).toBeInTheDocument();
    });

    it("panelInfoId is set as id", () => {
      // Given / When
      render(<WorkflowInfoPanel session={makeSession()} {...panelProps} />);
      // Then
      const panel = screen.getByRole("tabpanel");
      expect(panel).toHaveAttribute("id", "panel-info-1");
    });

    it("tabInfoId is set in aria-labelledby", () => {
      // Given / When
      render(<WorkflowInfoPanel session={makeSession()} {...panelProps} />);
      // Then
      const panel = screen.getByRole("tabpanel");
      expect(panel).toHaveAttribute("aria-labelledby", "tab-info-1");
    });
  });

  describe("configSource and baseDir display", () => {
    it("configSource is shown", () => {
      // Given / When
      render(
        <WorkflowInfoPanel
          session={makeSession({ configSource: "my-config.yaml" })}
          {...panelProps}
        />
      );
      // Then
      expect(screen.getByText("my-config.yaml")).toBeInTheDocument();
    });

    it("baseDir is shown", () => {
      // Given / When
      render(
        <WorkflowInfoPanel
          session={makeSession({ baseDir: "/projects/my-app" })}
          {...panelProps}
        />
      );
      // Then
      expect(screen.getByText("/projects/my-app")).toBeInTheDocument();
    });
  });

  describe("prUrl link display", () => {
    it("link is shown when prUrl is set", () => {
      // Given / When
      render(
        <WorkflowInfoPanel
          session={makeSession({ prUrl: "https://github.com/org/repo/pull/42" })}
          {...panelProps}
        />
      );
      // Then
      expect(screen.getByRole("link")).toBeInTheDocument();
    });

    it("link is not shown when prUrl is not set", () => {
      // Given / When
      render(
        <WorkflowInfoPanel
          session={makeSession({ prUrl: undefined })}
          {...panelProps}
        />
      );
      // Then
      expect(screen.queryByRole("link")).not.toBeInTheDocument();
    });
  });

  describe("Conditional display fields", () => {
    it("branch name is shown when worktreeBranch is set", () => {
      // Given / When
      render(
        <WorkflowInfoPanel
          session={makeSession({ worktreeBranch: "feature/my-branch" })}
          {...panelProps}
        />
      );
      // Then
      expect(screen.getByText("feature/my-branch")).toBeInTheDocument();
    });

    it("branch name is not shown when worktreeBranch is not set", () => {
      // Given / When
      render(
        <WorkflowInfoPanel
          session={makeSession({ worktreeBranch: undefined })}
          {...panelProps}
        />
      );
      // Then
      expect(screen.queryByText(/branch/i)).not.toBeInTheDocument();
    });

    it("error message is shown when phaseError is set", () => {
      // Given / When
      render(
        <WorkflowInfoPanel
          session={makeSession({ phaseError: "Something failed!" })}
          {...panelProps}
        />
      );
      // Then
      expect(screen.getByText("Something failed!")).toBeInTheDocument();
    });

    it("error section is not shown when phaseError is not set", () => {
      // Given / When
      render(
        <WorkflowInfoPanel
          session={makeSession({ phaseError: undefined })}
          {...panelProps}
        />
      );
      // Then
      expect(screen.queryByText("Something failed!")).not.toBeInTheDocument();
    });
  });
});
