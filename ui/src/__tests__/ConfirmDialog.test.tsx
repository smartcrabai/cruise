import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ConfirmDialog } from "../components/ConfirmDialog";

describe("ConfirmDialog", () => {
  const baseProps = {
    title: "Test Title",
    message: "Are you sure?",
    confirmLabel: "Confirm",
    onConfirm: vi.fn(),
    onCancel: vi.fn(),
  };

  beforeEach(() => {
    vi.clearAllMocks();
  });

  // --- Rendering ---

  it("renders title, message, confirm button, and cancel button", () => {
    // Given / When
    render(<ConfirmDialog {...baseProps} />);

    // Then
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByText("Test Title")).toBeInTheDocument();
    expect(screen.getByText("Are you sure?")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Confirm" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();
  });

  it("has aria-modal and aria-labelledby attributes for accessibility", () => {
    // Given / When
    render(<ConfirmDialog {...baseProps} />);

    // Then
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(dialog).toHaveAttribute("aria-labelledby");
  });

  // --- Interaction ---

  it("calls onConfirm when confirm button is clicked", async () => {
    // Given
    const onConfirm = vi.fn();
    render(<ConfirmDialog {...baseProps} onConfirm={onConfirm} />);

    // When
    await userEvent.click(screen.getByRole("button", { name: "Confirm" }));

    // Then
    expect(onConfirm).toHaveBeenCalledOnce();
  });

  it("calls onCancel when cancel button is clicked", async () => {
    // Given
    const onCancel = vi.fn();
    render(<ConfirmDialog {...baseProps} onCancel={onCancel} />);

    // When
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));

    // Then
    expect(onCancel).toHaveBeenCalledOnce();
  });

  it("does not call onConfirm when clicking cancel", async () => {
    // Given
    const onConfirm = vi.fn();
    render(<ConfirmDialog {...baseProps} onConfirm={onConfirm} />);

    // When
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));

    // Then
    expect(onConfirm).not.toHaveBeenCalled();
  });

  // --- Variant: destructive (default) ---

  it("applies red styling to confirm button by default (destructive variant)", () => {
    // Given / When
    render(<ConfirmDialog {...baseProps} />);

    // Then
    const confirmBtn = screen.getByRole("button", { name: "Confirm" });
    expect(confirmBtn.className).toContain("bg-red-600");
    expect(confirmBtn.className).not.toContain("bg-blue-600");
  });

  it("applies red styling to confirm button when variant is explicitly 'destructive'", () => {
    // Given / When
    render(<ConfirmDialog {...baseProps} variant="destructive" />);

    // Then
    expect(screen.getByRole("button", { name: "Confirm" }).className).toContain("bg-red-600");
  });

  // --- Variant: primary ---

  it("applies blue styling to confirm button when variant is 'primary'", () => {
    // Given / When
    render(<ConfirmDialog {...baseProps} variant="primary" />);

    // Then
    const confirmBtn = screen.getByRole("button", { name: "Confirm" });
    expect(confirmBtn.className).toContain("bg-blue-600");
    expect(confirmBtn.className).not.toContain("bg-red-600");
  });

});
