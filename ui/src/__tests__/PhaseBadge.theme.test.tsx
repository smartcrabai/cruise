/**
 * Tests for PhaseBadge system-theme (light/dark mode) colour adaptation.
 *
 * The plan requires every dark-only grayscale colour class to be replaced with
 * a light-mode base plus a `dark:` override. PhaseBadge is a simple, focused
 * component that exercises this requirement for the gray palette.
 *
 * These tests will fail until PhaseBadge.tsx is updated.
 */
import { describe, it, expect, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import { PhaseBadge } from "../components/PhaseBadge";

afterEach(cleanup);

describe("PhaseBadge: system theme colours", () => {
  describe("Draft phase", () => {
    it("uses a light-mode background base with a dark override", () => {
      // Given / When
      render(<PhaseBadge phase="Draft" />);
      const badge = screen.getByText("Draft");

      // Then: light background is visible in light mode, dark variant in dark mode
      expect(badge.className).toMatch(/\bbg-gray-100\/50\b/);
      expect(badge.className).toMatch(/\bdark:bg-gray-800\/50\b/);
    });

    it("uses a light-mode text colour base with a dark override", () => {
      // Given / When
      render(<PhaseBadge phase="Draft" />);
      const badge = screen.getByText("Draft");

      // Then
      expect(badge.className).toMatch(/\btext-gray-600\b/);
      expect(badge.className).toMatch(/\bdark:text-gray-400\b/);
    });
  });

  describe("Completed phase", () => {
    it("uses a light-mode background base with a dark override", () => {
      // Given / When
      render(<PhaseBadge phase="Completed" />);
      const badge = screen.getByText("Completed");

      // Then
      expect(badge.className).toMatch(/\bbg-gray-200\/50\b/);
      expect(badge.className).toMatch(/\bdark:bg-gray-700\/50\b/);
    });

    it("uses a light-mode text colour base with a dark override", () => {
      // Given / When
      render(<PhaseBadge phase="Completed" />);
      const badge = screen.getByText("Completed");

      // Then
      expect(badge.className).toMatch(/\btext-gray-800\b/);
      expect(badge.className).toMatch(/\bdark:text-gray-300\b/);
    });
  });

  describe("coloured phases remain readable in both modes", () => {
    it.each([
      ["Planned", "bg-blue-900/50"],
      ["Running", "bg-green-900/50"],
      ["Failed", "bg-red-900/50"],
      ["Awaiting Approval", "bg-yellow-900/50"],
      ["Awaiting Input", "bg-amber-900/50"],
      ["Suspended", "bg-orange-900/50"],
    ] as const)("%s keeps its dark background variant", (phase, expectedDarkBg) => {
      // Given / When
      render(<PhaseBadge phase={phase} />);
      const badge = screen.getByText(phase);

      // Then: coloured backgrounds rely on opacity and keep a dark variant
      expect(badge.className).toMatch(new RegExp(`\\b${expectedDarkBg.replace("/", "\\/")}\\b`));
    });
  });
});
