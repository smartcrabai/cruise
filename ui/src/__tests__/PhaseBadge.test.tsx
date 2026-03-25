import { describe, it, expect, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import { PhaseBadge } from "../components/PhaseBadge";

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

    it("always renders the text 'Awaiting Approval' regardless of planAvailable", () => {
      // Given / When
      const { rerender } = render(
        <PhaseBadge phase="Awaiting Approval" planAvailable={true} />,
      );
      expect(screen.getByText("Awaiting Approval")).toBeTruthy();

      rerender(<PhaseBadge phase="Awaiting Approval" planAvailable={false} />);
      expect(screen.getByText("Awaiting Approval")).toBeTruthy();

      rerender(<PhaseBadge phase="Awaiting Approval" />);
      expect(screen.getByText("Awaiting Approval")).toBeTruthy();
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

      rerender(<PhaseBadge phase="Suspended" />);
      expect(screen.getByText("Suspended")).toBeTruthy();
    });
  });
});
