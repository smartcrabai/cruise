import { render, screen, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AskEditor } from "./AskEditor";

afterEach(() => cleanup());

const baseProps = {
  question: "",
  onQuestionChange: vi.fn(),
  phase: "idle" as const,
  error: "",
  onSubmit: vi.fn(),
  onCancel: vi.fn(),
};

describe("AskEditor", () => {
  describe("Textarea display", () => {
    it("textarea with placeholder='Ask a question about the plan...' exists", () => {
      // Given / When
      render(<AskEditor {...baseProps} />);
      // Then
      expect(
        screen.getByPlaceholderText("Ask a question about the plan...")
      ).toBeInTheDocument();
    });
  });

  describe("Submit button disabled state", () => {
    it("Submit button is disabled when question is empty", () => {
      // Given / When
      render(<AskEditor {...baseProps} question="" />);
      // Then
      expect(screen.getByRole("button", { name: /submit/i })).toBeDisabled();
    });

    it("Submit button is disabled when question is whitespace only", () => {
      // Given / When
      render(<AskEditor {...baseProps} question="   " />);
      // Then
      expect(screen.getByRole("button", { name: /submit/i })).toBeDisabled();
    });

    it("Submit button is enabled when question has text", () => {
      // Given / When
      render(<AskEditor {...baseProps} question="What about step 2?" />);
      // Then
      expect(screen.getByRole("button", { name: /submit/i })).not.toBeDisabled();
    });

    it("Submit button is disabled when phase='submitting'", () => {
      // Given / When
      render(<AskEditor {...baseProps} question="some question" phase="submitting" />);
      // Then
      expect(screen.getByRole("button", { name: /asking/i })).toBeDisabled();
    });
  });

  describe("Display changes by phase", () => {
    it("Button label is 'Submit' when phase='idle'", () => {
      // Given / When
      render(<AskEditor {...baseProps} phase="idle" question="q" />);
      // Then
      expect(screen.getByRole("button", { name: "Submit" })).toBeInTheDocument();
    });

    it("Button label is 'Asking...' when phase='submitting'", () => {
      // Given / When
      render(<AskEditor {...baseProps} phase="submitting" question="q" />);
      // Then
      expect(screen.getByRole("button", { name: "Asking..." })).toBeInTheDocument();
    });
  });

  describe("Keyboard interaction", () => {
    it("onSubmit is called on Cmd+Enter", async () => {
      // Given
      const onSubmit = vi.fn();
      render(
        <AskEditor {...baseProps} question="Is this correct?" onSubmit={onSubmit} />
      );
      const textarea = screen.getByPlaceholderText("Ask a question about the plan...");

      // When
      await userEvent.click(textarea);
      await userEvent.keyboard("{Meta>}{Enter}{/Meta}");

      // Then
      expect(onSubmit).toHaveBeenCalledOnce();
    });

    it("onSubmit is called on Ctrl+Enter", async () => {
      // Given
      const onSubmit = vi.fn();
      render(
        <AskEditor {...baseProps} question="Is this correct?" onSubmit={onSubmit} />
      );
      const textarea = screen.getByPlaceholderText("Ask a question about the plan...");

      // When
      await userEvent.click(textarea);
      await userEvent.keyboard("{Control>}{Enter}{/Control}");

      // Then
      expect(onSubmit).toHaveBeenCalledOnce();
    });
  });

  describe("Error display", () => {
    it("Error message is not shown when error is empty", () => {
      // Given / When
      render(<AskEditor {...baseProps} error="" />);
      // Then
      expect(screen.queryByRole("paragraph")).not.toBeInTheDocument();
    });

    it("Error message is shown when error is set", () => {
      // Given / When
      render(<AskEditor {...baseProps} error="Something went wrong" />);
      // Then
      expect(screen.getByText("Something went wrong")).toBeInTheDocument();
    });
  });

  describe("Cancel button", () => {
    it("Cancel button is always visible", () => {
      // Given / When
      render(<AskEditor {...baseProps} />);
      // Then
      expect(screen.getByRole("button", { name: /cancel/i })).toBeInTheDocument();
    });

    it("onCancel is called when Cancel button is clicked", async () => {
      // Given
      const onCancel = vi.fn();
      render(<AskEditor {...baseProps} onCancel={onCancel} />);

      // When
      await userEvent.click(screen.getByRole("button", { name: /cancel/i }));

      // Then
      expect(onCancel).toHaveBeenCalledOnce();
    });
  });
});
