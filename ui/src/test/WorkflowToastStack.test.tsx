import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { WorkflowToastStack } from "../App";
import type { WorkflowToast } from "../App";

function makeToast(overrides: Partial<WorkflowToast> = {}): WorkflowToast {
  return {
    id: 1,
    kind: "completed",
    sessionInput: "test-task",
    ...overrides,
  };
}

describe("WorkflowToastStack", () => {
  describe("when toast list is empty", () => {
    it("renders nothing", () => {
      // Given: toasts is empty
      const { container } = render(
        <WorkflowToastStack toasts={[]} onDismiss={vi.fn()} />
      );

      // Then: DOM is empty
      expect(container.firstChild).toBeNull();
    });
  });

  describe("kind: 'completed' toast", () => {
    it("displays 'Completed' label", () => {
      // Given
      const toast = makeToast({ kind: "completed", sessionInput: "task-a" });

      // When
      render(<WorkflowToastStack toasts={[toast]} onDismiss={vi.fn()} />);

      // Then
      expect(screen.getByText("Completed")).toBeInTheDocument();
    });

    it("displays sessionInput as text", () => {
      // Given
      const toast = makeToast({ kind: "completed", sessionInput: "task-a" });

      // When
      render(<WorkflowToastStack toasts={[toast]} onDismiss={vi.fn()} />);

      // Then
      expect(screen.getByText("task-a")).toBeInTheDocument();
    });
  });

  describe("kind: 'input-required' toast", () => {
    it("displays 'Action required' label", () => {
      // Given
      const toast = makeToast({ kind: "input-required", sessionInput: "input-pending-task" });

      // When
      render(<WorkflowToastStack toasts={[toast]} onDismiss={vi.fn()} />);

      // Then
      expect(screen.getByText("Action required")).toBeInTheDocument();
    });
  });

  describe("kind: 'failed' toast", () => {
    it("displays 'Failed' label", () => {
      // Given
      const toast = makeToast({ kind: "failed", sessionInput: "failed-task" });

      // When
      render(<WorkflowToastStack toasts={[toast]} onDismiss={vi.fn()} />);

      // Then
      expect(screen.getByText("Failed")).toBeInTheDocument();
    });

    it("displays detail text", () => {
      // Given
      const toast = makeToast({
        kind: "failed",
        sessionInput: "failed-task",
        detail: "Command exited with code 1",
      });

      // When
      render(<WorkflowToastStack toasts={[toast]} onDismiss={vi.fn()} />);

      // Then
      expect(screen.getByText("Command exited with code 1")).toBeInTheDocument();
    });

    it("does not display detail element when detail is undefined", () => {
      // Given: no detail
      const toast = makeToast({ kind: "failed", sessionInput: "failed-task", detail: undefined });

      // When
      render(<WorkflowToastStack toasts={[toast]} onDismiss={vi.fn()} />);

      // Then: no detail element (only label and sessionInput)
      expect(screen.queryByTestId("toast-detail")).not.toBeInTheDocument();
    });
  });

  describe("multiple toasts display", () => {
    it("renders all toasts", () => {
      // Given: 3 kinds of toast
      const toasts: WorkflowToast[] = [
        makeToast({ id: 1, kind: "input-required", sessionInput: "task-1" }),
        makeToast({ id: 2, kind: "completed", sessionInput: "task-2" }),
        makeToast({ id: 3, kind: "failed", sessionInput: "task-3" }),
      ];

      // When
      render(<WorkflowToastStack toasts={toasts} onDismiss={vi.fn()} />);

      // Then: all sessionInputs are displayed
      expect(screen.getByText("task-1")).toBeInTheDocument();
      expect(screen.getByText("task-2")).toBeInTheDocument();
      expect(screen.getByText("task-3")).toBeInTheDocument();
    });
  });

  describe("kind: 'plan-ready' toast", () => {
    it("displays 'Plan ready' label", () => {
      // Given: an approval-ready notification toast
      const toast = makeToast({ kind: "plan-ready", sessionInput: "pending-approval-task" });

      // When
      render(<WorkflowToastStack toasts={[toast]} onDismiss={vi.fn()} />);

      // Then
      expect(screen.getByText("Plan ready")).toBeInTheDocument();
    });

    it("displays sessionInput text", () => {
      // Given
      const toast = makeToast({ kind: "plan-ready", sessionInput: "pending-approval-task" });

      // When
      render(<WorkflowToastStack toasts={[toast]} onDismiss={vi.fn()} />);

      // Then
      expect(screen.getByText("pending-approval-task")).toBeInTheDocument();
    });
  });

  describe("dismiss button", () => {
    it("clicking the x button calls onDismiss with the toast id", async () => {
      // Given
      const onDismiss = vi.fn();
      const toast = makeToast({ id: 42, kind: "completed", sessionInput: "task" });
      render(<WorkflowToastStack toasts={[toast]} onDismiss={onDismiss} />);

      // When
      const dismissBtn = screen.getByRole("button", { name: "Dismiss" });
      await userEvent.click(dismissBtn);

      // Then
      expect(onDismiss).toHaveBeenCalledOnce();
      expect(onDismiss).toHaveBeenCalledWith(42);
    });

    it("calls onDismiss with the correct id for each toast", async () => {
      // Given
      const onDismiss = vi.fn();
      const toasts: WorkflowToast[] = [
        makeToast({ id: 10, kind: "completed", sessionInput: "task-a" }),
        makeToast({ id: 20, kind: "failed", sessionInput: "task-b" }),
      ];
      render(<WorkflowToastStack toasts={toasts} onDismiss={onDismiss} />);

      // When: click x on the second toast
      const dismissBtns = screen.getAllByRole("button", { name: "Dismiss" });
      await userEvent.click(dismissBtns[1]);

      // Then: called with id=20
      expect(onDismiss).toHaveBeenCalledWith(20);
    });
  });
});
