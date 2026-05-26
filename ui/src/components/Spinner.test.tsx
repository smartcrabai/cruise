import { render, screen, cleanup } from "@testing-library/react";
import { afterEach, describe, it, expect } from "vitest";
import { Spinner } from "./Spinner";

afterEach(() => cleanup());

describe("Spinner", () => {
  describe("Accessibility role", () => {
    it("element with role=status exists with default props", () => {
      // Given / When
      render(<Spinner />);
      // Then
      expect(screen.getByRole("status")).toBeInTheDocument();
    });

    it("aria-label='Loading' is set with default props", () => {
      // Given / When
      render(<Spinner />);
      // Then
      expect(screen.getByLabelText("Loading")).toBeInTheDocument();
    });
  });

  describe("Custom color", () => {
    it("element with role=status exists when color is specified", () => {
      // Given / When
      render(<Spinner color="border-blue-500" />);
      // Then
      expect(screen.getByRole("status")).toBeInTheDocument();
    });

    it("default color class is applied when color is omitted", () => {
      // Given / When
      render(<Spinner />);
      // Then
      const el = screen.getByRole("status");
      expect(el.className).toContain("border-gray-400");
    });
  });
});
