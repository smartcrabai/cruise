import { render, screen, cleanup } from "@testing-library/react";
import { afterEach, describe, it, expect } from "vitest";
import { MarkdownViewer } from "./MarkdownViewer";

afterEach(() => cleanup());

describe("MarkdownViewer", () => {
  describe("Heading rendering", () => {
    it("H1 heading is rendered as text", () => {
      // Given
      const content = "# Hello World";
      // When
      render(<MarkdownViewer content={content} />);
      // Then
      expect(screen.getByRole("heading", { level: 1, name: "Hello World" })).toBeInTheDocument();
    });

    it("H2 heading is rendered as text", () => {
      // Given
      const content = "## Section Title";
      // When
      render(<MarkdownViewer content={content} />);
      // Then
      expect(screen.getByRole("heading", { level: 2, name: "Section Title" })).toBeInTheDocument();
    });
  });

  describe("GFM table", () => {
    it("GFM table is rendered with table role", () => {
      // Given
      const content = [
        "| Name | Value |",
        "| ---- | ----- |",
        "| foo  | bar   |",
      ].join("\n");
      // When
      render(<MarkdownViewer content={content} />);
      // Then
      expect(screen.getByRole("table")).toBeInTheDocument();
    });

    it("GFM table cell content is rendered", () => {
      // Given
      const content = [
        "| Name | Value |",
        "| ---- | ----- |",
        "| foo  | bar   |",
      ].join("\n");
      // When
      render(<MarkdownViewer content={content} />);
      // Then
      expect(screen.getByRole("cell", { name: "foo" })).toBeInTheDocument();
      expect(screen.getByRole("cell", { name: "bar" })).toBeInTheDocument();
    });
  });

  describe("className composition", () => {
    it("default prose class is applied when className is omitted", () => {
      // Given / When
      const { container } = render(<MarkdownViewer content="text" />);
      // Then
      const wrapper = container.firstChild as HTMLElement;
      expect(wrapper.className).toContain("prose");
    });

    it("additional class is appended to default when className is specified", () => {
      // Given / When
      const { container } = render(<MarkdownViewer content="text" className="p-6" />);
      // Then
      const wrapper = container.firstChild as HTMLElement;
      expect(wrapper.className).toContain("prose");
      expect(wrapper.className).toContain("p-6");
    });
  });

  describe("Inline elements", () => {
    it("inline code is rendered", () => {
      // Given
      const content = "Use `npm install` to install";
      // When
      render(<MarkdownViewer content={content} />);
      // Then
      expect(screen.getByText("npm install")).toBeInTheDocument();
    });

    it("bullet list items are rendered", () => {
      // Given
      const content = "- item one\n- item two";
      // When
      render(<MarkdownViewer content={content} />);
      // Then
      expect(screen.getByRole("list")).toBeInTheDocument();
      expect(screen.getByText("item one")).toBeInTheDocument();
      expect(screen.getByText("item two")).toBeInTheDocument();
    });
  });
});
