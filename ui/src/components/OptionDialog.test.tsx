import { render, screen, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ChoiceDto } from "../types";
import { OptionDialog } from "./OptionDialog";

afterEach(() => cleanup());

const selectorChoice: ChoiceDto = { label: "Continue", kind: "selector", next: "step-2" };
const anotherSelectorChoice: ChoiceDto = { label: "Abort", kind: "selector", next: "step-abort" };
const textInputChoice: ChoiceDto = { label: "Enter value", kind: "textInput", next: "step-input" };

describe("OptionDialog", () => {
  describe("Dialog accessibility", () => {
    it("element with role='dialog' exists", () => {
      // Given / When
      render(<OptionDialog choices={[selectorChoice]} onRespond={vi.fn()} />);
      // Then
      expect(screen.getByRole("dialog")).toBeInTheDocument();
    });

    it("aria-modal='true' is set", () => {
      // Given / When
      render(<OptionDialog choices={[selectorChoice]} onRespond={vi.fn()} />);
      // Then
      expect(screen.getByRole("dialog")).toHaveAttribute("aria-modal", "true");
    });

    it("'Choose an option' title is shown", () => {
      // Given / When
      render(<OptionDialog choices={[selectorChoice]} onRespond={vi.fn()} />);
      // Then
      expect(screen.getByText("Choose an option")).toBeInTheDocument();
    });
  });

  describe("selector kind choices", () => {
    it("onRespond({nextStep}) is called when selector button is clicked", async () => {
      // Given
      const onRespond = vi.fn();
      render(<OptionDialog choices={[selectorChoice]} onRespond={onRespond} />);

      // When
      await userEvent.click(screen.getByRole("button", { name: "Continue" }));

      // Then
      expect(onRespond).toHaveBeenCalledOnce();
      expect(onRespond).toHaveBeenCalledWith({ nextStep: "step-2" });
    });

    it("multiple selector choices are shown", () => {
      // Given / When
      render(
        <OptionDialog
          choices={[selectorChoice, anotherSelectorChoice]}
          onRespond={vi.fn()}
        />
      );
      // Then
      expect(screen.getByRole("button", { name: "Continue" })).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Abort" })).toBeInTheDocument();
    });
  });

  describe("textInput kind input form", () => {
    it("label and text input field are shown", () => {
      // Given / When
      render(<OptionDialog choices={[textInputChoice]} onRespond={vi.fn()} />);
      // Then
      expect(screen.getByLabelText("Enter value")).toBeInTheDocument();
    });

    it("onRespond({nextStep, textInput}) is called when Submit button is clicked", async () => {
      // Given
      const onRespond = vi.fn();
      render(<OptionDialog choices={[textInputChoice]} onRespond={onRespond} />);
      const input = screen.getByLabelText("Enter value");

      // When
      await userEvent.type(input, "my answer");
      await userEvent.click(screen.getByRole("button", { name: "Submit" }));

      // Then
      expect(onRespond).toHaveBeenCalledOnce();
      expect(onRespond).toHaveBeenCalledWith({ nextStep: "step-input", textInput: "my answer" });
    });

    it("onRespond({nextStep, textInput}) is called on Enter key", async () => {
      // Given
      const onRespond = vi.fn();
      render(<OptionDialog choices={[textInputChoice]} onRespond={onRespond} />);
      const input = screen.getByLabelText("Enter value");

      // When
      await userEvent.type(input, "my answer");
      await userEvent.keyboard("{Enter}");

      // Then
      expect(onRespond).toHaveBeenCalledOnce();
      expect(onRespond).toHaveBeenCalledWith({ nextStep: "step-input", textInput: "my answer" });
    });

    it("Submit button is disabled when text input is empty", () => {
      // Given / When
      render(<OptionDialog choices={[textInputChoice]} onRespond={vi.fn()} />);

      // Then
      expect(screen.getByRole("button", { name: "Submit" })).toBeDisabled();
    });
  });

  describe("plan property", () => {
    it("content is rendered when plan is specified", () => {
      // Given
      const plan = "# My Plan\n\nSome content here.";
      // When
      render(<OptionDialog choices={[selectorChoice]} plan={plan} onRespond={vi.fn()} />);
      // Then
      expect(screen.getByRole("heading", { level: 1, name: "My Plan" })).toBeInTheDocument();
    });

    it("display is not broken when plan is not specified", () => {
      // Given / When
      render(<OptionDialog choices={[selectorChoice]} onRespond={vi.fn()} />);
      // Then
      expect(screen.getByRole("dialog")).toBeInTheDocument();
    });
  });
});
