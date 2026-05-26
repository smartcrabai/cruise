import { render, screen, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SettingsModal } from "./SettingsModal";

afterEach(() => cleanup());

describe("SettingsModal", () => {
  describe("Dialog accessibility", () => {
    it("element with role='dialog' exists", () => {
      // Given / When
      render(<SettingsModal initialParallelism={2} onSave={vi.fn()} onClose={vi.fn()} />);
      // Then
      expect(screen.getByRole("dialog")).toBeInTheDocument();
    });

    it("aria-modal='true' is set", () => {
      // Given / When
      render(<SettingsModal initialParallelism={2} onSave={vi.fn()} onClose={vi.fn()} />);
      // Then
      expect(screen.getByRole("dialog")).toHaveAttribute("aria-modal", "true");
    });

    it("'Settings' title is shown", () => {
      // Given / When
      render(<SettingsModal initialParallelism={2} onSave={vi.fn()} onClose={vi.fn()} />);
      // Then
      expect(screen.getByRole("heading", { name: "Settings" })).toBeInTheDocument();
    });
  });

  describe("Initial value display", () => {
    it("initialParallelism is shown in the number input", () => {
      // Given / When
      render(<SettingsModal initialParallelism={4} onSave={vi.fn()} onClose={vi.fn()} />);
      // Then
      const input = screen.getByLabelText(/run all parallelism/i) as HTMLInputElement;
      expect(input.value).toBe("4");
    });
  });

  describe("Validation", () => {
    it("'Must be at least 1' error is shown when saving with value 0", async () => {
      // Given
      render(<SettingsModal initialParallelism={2} onSave={vi.fn()} onClose={vi.fn()} />);
      const input = screen.getByLabelText(/run all parallelism/i);

      // When
      await userEvent.clear(input);
      await userEvent.type(input, "0");
      await userEvent.click(screen.getByRole("button", { name: /save/i }));

      // Then
      expect(screen.getByText("Must be at least 1")).toBeInTheDocument();
    });

    it("'Must be at least 1' error is shown when saving with negative value", async () => {
      // Given
      render(<SettingsModal initialParallelism={2} onSave={vi.fn()} onClose={vi.fn()} />);
      const input = screen.getByLabelText(/run all parallelism/i);

      // When
      await userEvent.clear(input);
      await userEvent.type(input, "-1");
      await userEvent.click(screen.getByRole("button", { name: /save/i }));

      // Then
      expect(screen.getByText("Must be at least 1")).toBeInTheDocument();
    });

    it("error is shown and onSave is not called when saving with empty input", async () => {
      // Given
      const onSave = vi.fn();
      render(<SettingsModal initialParallelism={2} onSave={onSave} onClose={vi.fn()} />);
      const input = screen.getByLabelText(/run all parallelism/i);

      // When
      await userEvent.clear(input);
      await userEvent.click(screen.getByRole("button", { name: /save/i }));

      // Then
      expect(onSave).not.toHaveBeenCalled();
      expect(screen.getByText("Must be at least 1")).toBeInTheDocument();
    });
  });

  describe("Save processing", () => {
    it("onSave({runAllParallelism}) is called when saving with valid value", async () => {
      // Given
      const onSave = vi.fn().mockResolvedValue(undefined);
      const onClose = vi.fn();
      render(<SettingsModal initialParallelism={2} onSave={onSave} onClose={onClose} />);
      const input = screen.getByLabelText(/run all parallelism/i);

      // When
      await userEvent.clear(input);
      await userEvent.type(input, "3");
      await userEvent.click(screen.getByRole("button", { name: /save/i }));

      // Then
      expect(onSave).toHaveBeenCalledWith({ runAllParallelism: 3 });
      expect(onClose).toHaveBeenCalledTimes(1);
    });

    it("Save button shows 'Saving...' and becomes disabled while saving", async () => {
      // Given: onSave returns a Promise that never resolves
      let resolvePromise!: () => void;
      const onSave = vi.fn().mockImplementation(
        () => new Promise<void>((res) => { resolvePromise = res; })
      );
      render(<SettingsModal initialParallelism={2} onSave={onSave} onClose={vi.fn()} />);

      // When
      await userEvent.click(screen.getByRole("button", { name: /save/i }));

      // Then: button shows 'Saving...' and is disabled
      const savingBtn = await screen.findByRole("button", { name: /saving/i });
      expect(savingBtn).toBeDisabled();

      // cleanup
      resolvePromise();
    });

    it("Cancel button is also disabled while saving", async () => {
      // Given
      let resolvePromise!: () => void;
      const onSave = vi.fn().mockImplementation(
        () => new Promise<void>((res) => { resolvePromise = res; })
      );
      render(<SettingsModal initialParallelism={2} onSave={onSave} onClose={vi.fn()} />);

      // When
      await userEvent.click(screen.getByRole("button", { name: /^save$/i }));

      // Then
      const cancelBtn = screen.getByRole("button", { name: /cancel/i });
      expect(cancelBtn).toBeDisabled();

      // cleanup
      resolvePromise();
    });
  });

  describe("Cancel", () => {
    it("onClose is called when Cancel button is clicked", async () => {
      // Given
      const onClose = vi.fn();
      render(<SettingsModal initialParallelism={2} onSave={vi.fn()} onClose={onClose} />);

      // When
      await userEvent.click(screen.getByRole("button", { name: /cancel/i }));

      // Then
      expect(onClose).toHaveBeenCalledOnce();
    });
  });
});
