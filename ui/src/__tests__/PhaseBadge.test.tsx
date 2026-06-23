import { describe, it, expect, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import { PhaseBadge, PLANNING_LABEL, FIXING_LABEL } from "../components/PhaseBadge";

// aria-label used by the blue "approve ready" indicator inside PhaseBadge
const PLAN_READY_LABEL = "plan ready for approval";

afterEach(() => {
  cleanup();
});

describe("PhaseBadge", () => {
  describe("Awaiting Approval phase", () => {
    it("shows blue dot indicator when planAvailable is true", () => {
      // Given / When
      render(<PhaseBadge phase="Awaiting Approval" planAvailable={true} />);

      // Then
      expect(screen.getByLabelText(PLAN_READY_LABEL)).toBeTruthy();
    });

    it("does not show blue dot indicator when planAvailable is false", () => {
      // Given / When
      render(<PhaseBadge phase="Awaiting Approval" planAvailable={false} />);

      // Then
      expect(screen.queryByLabelText(PLAN_READY_LABEL)).toBeNull();
    });

    it("does not show blue dot indicator when planAvailable is undefined (safe default)", () => {
      // Given / When
      render(<PhaseBadge phase="Awaiting Approval" />);

      // Then
      expect(screen.queryByLabelText(PLAN_READY_LABEL)).toBeNull();
    });

    it("renders 'Awaiting Approval' text when planAvailable is true", () => {
      // Given / When
      render(<PhaseBadge phase="Awaiting Approval" planAvailable={true} />);

      // Then
      expect(screen.getByText("Awaiting Approval")).toBeTruthy();
    });

    it("renders 'Awaiting Approval' text even when planAvailable is false", () => {
      // Given / When
      render(<PhaseBadge phase="Awaiting Approval" planAvailable={false} />);

      // Then: planning waits use the distinct Awaiting Input phase instead
      expect(screen.getByText("Awaiting Approval")).toBeTruthy();
      expect(screen.queryByText(PLANNING_LABEL)).toBeNull();
    });

    it("renders 'Awaiting Approval' text when planAvailable is undefined", () => {
      // Given / When
      render(<PhaseBadge phase="Awaiting Approval" />);

      // Then: missing plan availability no longer masquerades as Planning
      expect(screen.getByText("Awaiting Approval")).toBeTruthy();
      expect(screen.queryByText(PLANNING_LABEL)).toBeNull();
    });
  });

  describe("other phases - blue dot must not appear", () => {
    it("does not show blue dot for Planned even when planAvailable is true", () => {
      // Given / When
      render(<PhaseBadge phase="Planned" planAvailable={true} />);

      // Then: planAvailable is irrelevant for non-Awaiting Approval phases
      expect(screen.queryByLabelText(PLAN_READY_LABEL)).toBeNull();
    });

    it("does not show blue dot for Running", () => {
      // Given / When
      render(<PhaseBadge phase="Running" planAvailable={true} />);

      // Then
      expect(screen.queryByLabelText(PLAN_READY_LABEL)).toBeNull();
    });

    it("does not show blue dot for Completed", () => {
      // Given / When
      render(<PhaseBadge phase="Completed" planAvailable={true} />);

      // Then
      expect(screen.queryByLabelText(PLAN_READY_LABEL)).toBeNull();
    });

    it("does not show blue dot for Failed", () => {
      // Given / When
      render(<PhaseBadge phase="Failed" planAvailable={true} />);

      // Then
      expect(screen.queryByLabelText(PLAN_READY_LABEL)).toBeNull();
    });

    it("does not show blue dot for Suspended", () => {
      // Given / When
      render(<PhaseBadge phase="Suspended" planAvailable={true} />);

      // Then
      expect(screen.queryByLabelText(PLAN_READY_LABEL)).toBeNull();
    });

    it("does not show approval-ready dot for Awaiting Input (it shows its own input-required dot)", () => {
      // Given: the planning agent is blocked on user input rather than ready for approval
      // When
      render(<PhaseBadge phase="Awaiting Input" planAvailable={true} />);

      // Then: the approval-ready indicator is absent; a separate input-required indicator is shown instead
      expect(screen.queryByLabelText(PLAN_READY_LABEL)).toBeNull();
    });

    it("renders correct label text for each phase", () => {
      // Given / When / Then: text content matches the phase name
      const { rerender } = render(<PhaseBadge phase="Planned" />);
      expect(screen.getByText("Planned")).toBeTruthy();

      rerender(<PhaseBadge phase="Running" />);
      expect(screen.getByText("Running")).toBeTruthy();

      rerender(<PhaseBadge phase="Completed" />);
      expect(screen.getByText("Completed")).toBeTruthy();

      rerender(<PhaseBadge phase="Failed" />);
      expect(screen.getByText("Failed")).toBeTruthy();

      rerender(<PhaseBadge phase="Awaiting Input" />);
      expect(screen.getByText("Awaiting Input")).toBeTruthy();

      rerender(<PhaseBadge phase="Suspended" />);
      expect(screen.getByText("Suspended")).toBeTruthy();
    });
  });

  describe("Awaiting Input phase", () => {
    const INPUT_READY_LABEL = "user input required";

    it("shows blue dot for Awaiting Input phase", () => {
      // Given / When
      render(<PhaseBadge phase="Awaiting Input" />);

      // Then: the input-required indicator is present
      expect(screen.getByLabelText(INPUT_READY_LABEL)).toBeTruthy();
    });

    it("shows blue dot for Awaiting Input regardless of planAvailable", () => {
      // Given / When
      render(<PhaseBadge phase="Awaiting Input" planAvailable={false} />);

      // Then: planAvailable does not gate the input-required dot
      expect(screen.getByLabelText(INPUT_READY_LABEL)).toBeTruthy();
    });

    it("input-required dot uses distinct aria-label from the approval-ready dot", () => {
      // Given / When: Awaiting Input session
      render(<PhaseBadge phase="Awaiting Input" />);

      // Then: aria-label distinguishes the two dots
      expect(screen.getByLabelText(INPUT_READY_LABEL)).toBeTruthy();
      expect(screen.queryByLabelText(PLAN_READY_LABEL)).toBeNull();
    });

    it("renders 'Awaiting Input' text label", () => {
      // Given / When
      render(<PhaseBadge phase="Awaiting Input" />);

      // Then
      expect(screen.getByText("Awaiting Input")).toBeTruthy();
    });
  });

  describe("Draft phase", () => {
    it("renders 'Draft' text", () => {
      // Given / When
      render(<PhaseBadge phase="Draft" />);

      // Then: displays "Draft" label
      expect(screen.getByText("Draft")).toBeTruthy();
    });

    it("does not show blue dot for Draft", () => {
      // Given / When
      render(<PhaseBadge phase="Draft" planAvailable={true} />);

      // Then: Draft has no approval-ready indicator (no plan yet)
      expect(screen.queryByLabelText(PLAN_READY_LABEL)).toBeNull();
    });

    it("does not show 'Planning' or 'Awaiting Approval' text for Draft", () => {
      // Given / When
      render(<PhaseBadge phase="Draft" />);

      // Then: Draft uses its own label, not re-using Awaiting Approval display logic
      expect(screen.queryByText(PLANNING_LABEL)).toBeNull();
      expect(screen.queryByText("Awaiting Approval")).toBeNull();
    });
  });

  describe("Draft planning (in-flight override)", () => {
    it("renders PLANNING_LABEL when phase is Draft and fixing is true", () => {
      // Given: a Draft session for which plan generation is now in-flight
      // When
      render(<PhaseBadge phase="Draft" fixing={true} />);

      // Then: "Planning" is shown to communicate the in-flight state
      expect(screen.getByText(PLANNING_LABEL)).toBeTruthy();
    });

    it("does not show the blue approval-ready dot when Draft + fixing", () => {
      // Given / When
      render(<PhaseBadge phase="Draft" planAvailable={true} fixing={true} />);

      // Then: the approval dot must never appear for a Draft session
      expect(screen.queryByLabelText(PLAN_READY_LABEL)).toBeNull();
    });

    it("renders 'Draft' text when fixing is false (no-op)", () => {
      // Given / When
      render(<PhaseBadge phase="Draft" fixing={false} />);

      // Then: non-in-flight Draft session keeps its label
      expect(screen.getByText("Draft")).toBeTruthy();
      expect(screen.queryByText(PLANNING_LABEL)).toBeNull();
    });
  });

  describe("fixing override", () => {
    it("renders 'Fixing' text when fixing is true, overriding the Awaiting Approval label", () => {
      // Given: an Awaiting Approval session with a plan, but fix is currently in progress
      // When
      render(<PhaseBadge phase="Awaiting Approval" planAvailable={true} fixing={true} />);

      // Then: "Fixing" is displayed instead of "Awaiting Approval"
      expect(screen.getByText(FIXING_LABEL)).toBeTruthy();
      expect(screen.queryByText("Awaiting Approval")).toBeNull();
    });

    it("suppresses the blue dot when fixing is true even though planAvailable is true", () => {
      // Given: plan is available, but a fix is currently running
      // When
      render(<PhaseBadge phase="Awaiting Approval" planAvailable={true} fixing={true} />);

      // Then: the approval-ready indicator is hidden (fix is not yet ready for approval)
      expect(screen.queryByLabelText(PLAN_READY_LABEL)).toBeNull();
    });

    it("fixing=false produces the same output as omitting the prop (no-op)", () => {
      // Given: plan is available, fixing is explicitly false
      // When
      render(<PhaseBadge phase="Awaiting Approval" planAvailable={true} fixing={false} />);

      // Then: normal Awaiting Approval behavior — label and blue dot are both shown
      expect(screen.getByText("Awaiting Approval")).toBeTruthy();
      expect(screen.getByLabelText(PLAN_READY_LABEL)).toBeTruthy();
    });
  });
});
