import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { PublishIssueDialog } from "../components/PublishIssueDialog";

describe("PublishIssueDialog", () => {
  const baseProps = {
    sessionId: "session-1",
    onConfirm: vi.fn().mockResolvedValue(undefined),
    onCancel: vi.fn(),
  };

  beforeEach(() => {
    vi.clearAllMocks();
  });

  // --- Rendering ---------------------------------------------------------------

  it("renders title, session id, checkbox, and action buttons", () => {
    // Given / When
    render(<PublishIssueDialog {...baseProps} />);

    // Then
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByText("Publish as GitHub Issue")).toBeInTheDocument();
    expect(screen.getByText(/session-1/)).toBeInTheDocument();
    expect(screen.getByRole("checkbox")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Publish" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();
  });

  it("mentions that the issue body is plan.md, unchanged", () => {
    // Given / When
    render(<PublishIssueDialog {...baseProps} />);

    // Then: the copy makes explicit that plan.md is not modified before publishing
    expect(screen.getByText(/plan\.md/)).toBeInTheDocument();
  });

  it("labels the checkbox as posting a separate `@cruise run` comment", () => {
    // Given / When
    render(<PublishIssueDialog {...baseProps} />);

    // Then: the checkbox describes triggering the Action via a follow-up comment,
    // not mentioning @cruise inside the issue body itself
    expect(screen.getByText(/@cruise run/)).toBeInTheDocument();
  });

  // --- defaultTriggerCruise: checkbox initial state -----------------------------

  describe("defaultTriggerCruise", () => {
    it("checkbox is unchecked by default when defaultTriggerCruise is omitted", () => {
      // Given: the Awaiting Approval use case, which does not pass defaultTriggerCruise
      render(<PublishIssueDialog {...baseProps} />);

      // Then
      expect(screen.getByRole("checkbox")).not.toBeChecked();
    });

    it("checkbox is unchecked by default when defaultTriggerCruise is false", () => {
      // Given
      render(<PublishIssueDialog {...baseProps} defaultTriggerCruise={false} />);

      // Then
      expect(screen.getByRole("checkbox")).not.toBeChecked();
    });

    it("checkbox is checked by default when defaultTriggerCruise is true", () => {
      // Given: the Planned use case, where publishing replaces running locally
      render(<PublishIssueDialog {...baseProps} defaultTriggerCruise={true} />);

      // Then
      expect(screen.getByRole("checkbox")).toBeChecked();
    });
  });

  // --- onConfirm argument -------------------------------------------------------

  describe("onConfirm argument", () => {
    it("calls onConfirm with false when Publish is clicked without touching the checkbox", async () => {
      // Given
      const onConfirm = vi.fn().mockResolvedValue(undefined);
      render(<PublishIssueDialog {...baseProps} onConfirm={onConfirm} />);

      // When
      await userEvent.click(screen.getByRole("button", { name: "Publish" }));

      // Then
      expect(onConfirm).toHaveBeenCalledWith(false);
    });

    it("calls onConfirm with true after checking the box and clicking Publish", async () => {
      // Given
      const onConfirm = vi.fn().mockResolvedValue(undefined);
      render(<PublishIssueDialog {...baseProps} onConfirm={onConfirm} />);

      // When
      await userEvent.click(screen.getByRole("checkbox"));
      await userEvent.click(screen.getByRole("button", { name: "Publish" }));

      // Then
      expect(onConfirm).toHaveBeenCalledWith(true);
    });

    it("calls onConfirm with true by default when defaultTriggerCruise is true and Publish is clicked without touching the checkbox", async () => {
      // Given: Planned session flow -- default ON, user just clicks Publish
      const onConfirm = vi.fn().mockResolvedValue(undefined);
      render(
        <PublishIssueDialog {...baseProps} onConfirm={onConfirm} defaultTriggerCruise={true} />
      );

      // When
      await userEvent.click(screen.getByRole("button", { name: "Publish" }));

      // Then
      expect(onConfirm).toHaveBeenCalledWith(true);
    });

    it("calls onConfirm with false after unchecking a defaulted-on checkbox", async () => {
      // Given: Planned session flow, but the user opts out of the @cruise run comment
      const onConfirm = vi.fn().mockResolvedValue(undefined);
      render(
        <PublishIssueDialog {...baseProps} onConfirm={onConfirm} defaultTriggerCruise={true} />
      );

      // When
      await userEvent.click(screen.getByRole("checkbox"));
      await userEvent.click(screen.getByRole("button", { name: "Publish" }));

      // Then
      expect(onConfirm).toHaveBeenCalledWith(false);
    });
  });

  // --- Cancel --------------------------------------------------------------------

  describe("Cancel", () => {
    it("calls onCancel when the Cancel button is clicked", async () => {
      // Given
      const onCancel = vi.fn();
      render(<PublishIssueDialog {...baseProps} onCancel={onCancel} />);

      // When
      await userEvent.click(screen.getByRole("button", { name: "Cancel" }));

      // Then
      expect(onCancel).toHaveBeenCalledOnce();
    });

    it("calls onCancel when Escape is pressed", async () => {
      // Given
      const onCancel = vi.fn();
      render(<PublishIssueDialog {...baseProps} onCancel={onCancel} />);

      // When
      await userEvent.keyboard("{Escape}");

      // Then
      expect(onCancel).toHaveBeenCalledOnce();
    });

    it("does not call onConfirm when Cancel is clicked", async () => {
      // Given
      const onConfirm = vi.fn().mockResolvedValue(undefined);
      render(<PublishIssueDialog {...baseProps} onConfirm={onConfirm} />);

      // When
      await userEvent.click(screen.getByRole("button", { name: "Cancel" }));

      // Then
      expect(onConfirm).not.toHaveBeenCalled();
    });
  });

  // --- Error handling --------------------------------------------------------------

  describe("when onConfirm rejects", () => {
    it("shows the error message and re-enables the Publish button", async () => {
      // Given
      const onConfirm = vi.fn().mockRejectedValue(new Error("gh issue create failed"));
      render(<PublishIssueDialog {...baseProps} onConfirm={onConfirm} />);

      // When
      await userEvent.click(screen.getByRole("button", { name: "Publish" }));

      // Then
      expect(await screen.findByText(/gh issue create failed/)).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Publish" })).not.toBeDisabled();
    });
  });
});
