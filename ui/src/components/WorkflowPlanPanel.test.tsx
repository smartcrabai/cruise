import { render, screen, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { WorkflowPlanPanel } from "./WorkflowPlanPanel";

afterEach(() => cleanup());

const baseProps = {
  panelPlanId: "panel-plan-1",
  tabPlanId: "tab-plan-1",
  askResponse: "",
  planLoading: false,
  planContent: "",
};

describe("WorkflowPlanPanel", () => {
  describe("role / accessibility", () => {
    it("element with role='tabpanel' exists", () => {
      // Given / When
      render(<WorkflowPlanPanel {...baseProps} />);
      // Then
      expect(screen.getByRole("tabpanel")).toBeInTheDocument();
    });

    it("panelPlanId is set as id", () => {
      // Given / When
      render(<WorkflowPlanPanel {...baseProps} />);
      // Then
      expect(screen.getByRole("tabpanel")).toHaveAttribute("id", "panel-plan-1");
    });
  });

  describe("planLoading state", () => {
    it("Spinner(role=status) is shown when planLoading=true", () => {
      // Given / When
      render(<WorkflowPlanPanel {...baseProps} planLoading={true} />);
      // Then
      expect(screen.getByRole("status")).toBeInTheDocument();
    });

    it("'Loading plan...' text is shown when planLoading=true", () => {
      // Given / When
      render(<WorkflowPlanPanel {...baseProps} planLoading={true} />);
      // Then
      expect(screen.getByText("Loading plan...")).toBeInTheDocument();
    });

    it("Spinner is not shown when planLoading=false", () => {
      // Given / When
      render(<WorkflowPlanPanel {...baseProps} planLoading={false} planContent="# Plan" />);
      // Then
      expect(screen.queryByRole("status")).not.toBeInTheDocument();
    });
  });

  describe("planContent display", () => {
    it("Markdown content is shown when planContent is present", () => {
      // Given / When
      render(
        <WorkflowPlanPanel
          {...baseProps}
          planContent={"# Implementation Plan\n\nStep 1: Do something"}
        />
      );
      // Then
      expect(screen.getByRole("heading", { level: 1, name: "Implementation Plan" })).toBeInTheDocument();
      expect(screen.getByText("Step 1: Do something")).toBeInTheDocument();
    });

    it("'No plan available.' is shown when planContent is empty", () => {
      // Given / When
      render(<WorkflowPlanPanel {...baseProps} planContent="" />);
      // Then
      expect(screen.getByText("No plan available.")).toBeInTheDocument();
    });

    it("Spinner takes priority even when planContent is present if planLoading=true", () => {
      // Given / When
      render(
        <WorkflowPlanPanel
          {...baseProps}
          planLoading={true}
          planContent="# Existing plan"
        />
      );
      // Then
      expect(screen.getByText("Loading plan...")).toBeInTheDocument();
      expect(screen.queryByText("Existing plan")).not.toBeInTheDocument();
    });
  });

  describe("askResponse banner", () => {
    it("Answer section is shown when askResponse is set", () => {
      // Given / When
      render(
        <WorkflowPlanPanel
          {...baseProps}
          askResponse="The answer is 42."
        />
      );
      // Then
      expect(screen.getByText("Answer")).toBeInTheDocument();
      expect(screen.getByText("The answer is 42.")).toBeInTheDocument();
    });

    it("Answer section is not shown when askResponse is empty", () => {
      // Given / When
      render(<WorkflowPlanPanel {...baseProps} askResponse="" />);
      // Then
      expect(screen.queryByText("Answer")).not.toBeInTheDocument();
    });
  });
});
