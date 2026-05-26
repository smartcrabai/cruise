import { render, screen, cleanup } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { EmptyState } from "./EmptyState";

afterEach(() => cleanup());

describe("EmptyState", () => {
  it("shows 'Select a session from the sidebar' text", () => {
    // Given / When
    render(<EmptyState />);
    // Then
    expect(screen.getByText("Select a session from the sidebar")).toBeInTheDocument();
  });
});
