/**
 * Tests for ConfirmDialog system-theme (light/dark mode) colour adaptation.
 *
 * ConfirmDialog is a representative modal component that uses the full grayscale
 * palette (background, border, text, hover). The plan requires every dark-only
 * grayscale colour class to be replaced with a light-mode base plus a `dark:`
 * override.
 *
 * These tests will fail until ConfirmDialog.tsx is updated.
 */
import { describe, it, expect, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import { ConfirmDialog } from "../components/ConfirmDialog";

afterEach(cleanup);

function renderDialog() {
  return render(
    <ConfirmDialog
      title="Delete item?"
      message="This cannot be undone."
      confirmLabel="Delete"
      variant="destructive"
      onConfirm={() => {}}
      onCancel={() => {}}
    />,
  );
}

describe("ConfirmDialog: system theme colours", () => {
  it("dialog panel has light background base and dark override", () => {
    // Given / When
    renderDialog();
    const dialog = screen.getByRole("dialog");

    // Then
    expect(dialog.className).toMatch(/\bbg-white\b/);
    expect(dialog.className).toMatch(/\bdark:bg-gray-900\b/);
  });

  it("dialog panel has light border base and dark override", () => {
    // Given / When
    renderDialog();
    const dialog = screen.getByRole("dialog");

    // Then
    expect(dialog.className).toMatch(/\bborder-gray-200\b/);
    expect(dialog.className).toMatch(/\bdark:border-gray-700\b/);
  });

  it("dialog title has light text base and dark override", () => {
    // Given / When
    renderDialog();
    const title = screen.getByText("Delete item?");

    // Then
    expect(title.className).toMatch(/\btext-gray-900\b/);
    expect(title.className).toMatch(/\bdark:text-gray-100\b/);
  });

  it("dialog message has light text base and dark override", () => {
    // Given / When
    renderDialog();
    const message = screen.getByText("This cannot be undone.");

    // Then
    expect(message.className).toMatch(/\btext-gray-500\b/);
    expect(message.className).toMatch(/\bdark:text-gray-400\b/);
  });

  it("cancel button has light hover base and dark override", () => {
    // Given / When
    renderDialog();
    const cancelButton = screen.getByRole("button", { name: /cancel/i });

    // Then
    expect(cancelButton.className).toMatch(/\bhover:bg-gray-200\b/);
    expect(cancelButton.className).toMatch(/\bdark:hover:bg-gray-800\b/);
  });

  it("modal backdrop has a visible light-mode overlay", () => {
    // Given / When
    const { container } = renderDialog();
    const backdrop = container.firstElementChild as HTMLElement;

    // Then: backdrop must not be fully opaque black in light mode
    expect(backdrop.className).toMatch(/\bbg-black\/\d+\b/);
    expect(backdrop.className).not.toMatch(/\bbg-black\b(?!\/)/);
  });
});
