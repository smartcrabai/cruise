/**
 * Tests for the system-theme (light/dark mode) configuration.
 *
 * These tests verify the global theme wiring described in the plan:
 *   - The HTML document must not hardcode a "dark" class.
 *   - Tailwind's dark variant must be driven by the OS `prefers-color-scheme`
 *     media query instead of a manual `.dark` class.
 *
 * They will fail until the production files are updated.
 */
/// <reference types="node" />
import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const uiRoot = resolve(__dirname, "../..");
const projectRoot = resolve(uiRoot, "..");

function readUiFile(relativePath: string): string {
  return readFileSync(resolve(uiRoot, relativePath), "utf-8");
}

function readProjectFile(relativePath: string): string {
  return readFileSync(resolve(projectRoot, relativePath), "utf-8");
}

describe("System theme configuration", () => {
  describe("index.html", () => {
    it("does not hardcode the dark class on the html element", () => {
      // Given: the entry HTML file
      const html = readUiFile("index.html");

      // Then: the html element must not contain class="... dark ..."
      expect(html).not.toMatch(/<html[^>]*\sclass=["'][^"']*\bdark\b[^"']*["']/i);
    });
  });

  describe("index.css", () => {
    it("defines the dark variant using prefers-color-scheme", () => {
      // Given: the Tailwind entry CSS file
      const css = readUiFile("src/index.css");

      // Then: the dark variant is media-query based, not class based
      expect(css).toMatch(
        /@custom-variant\s+dark\s*\(\s*@media\s*\(\s*prefers-color-scheme:\s*dark\s*\)\s*\)/is,
      );
    });

    it("does not use the manual .dark class selector for the dark variant", () => {
      // Given: the Tailwind entry CSS file
      const css = readUiFile("src/index.css");

      // Then: the old class-based selector must be gone
      expect(css).not.toMatch(/@custom-variant\s+dark\s*\([^)]*\.dark[^)]*\)/is);
    });
  });

  describe("tauri.conf.json", () => {
    it("does not pin the native title bar to a fixed theme", () => {
      // Given: the Tauri window configuration
      const config = readProjectFile("src-tauri/tauri.conf.json");

      // Then: no explicit "theme" value is set (leaving it as OS auto-follow)
      const windowsMatch = config.match(/"windows"\s*:\s*\[([\s\S]*?)\]/);
      expect(windowsMatch).not.toBeNull();
      const windowBlock = windowsMatch![1];
      expect(windowBlock).not.toMatch(/"theme"\s*:/i);
    });
  });
});
