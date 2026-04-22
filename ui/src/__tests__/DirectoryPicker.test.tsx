import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, act, cleanup, fireEvent } from "@testing-library/react";
import { useState } from "react";
import { DirectoryPicker } from "../components/DirectoryPicker";
import * as commands from "../lib/commands";
import type { DirEntry } from "../types";

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("../lib/commands", () => ({
  listDirectory: vi.fn(),
}));

// Controlled wrapper so that fireEvent.change drives value through the real
// onChange path (not just a direct prop update from the parent).
function Controlled({ initialValue }: { initialValue: string }) {
  const [value, setValue] = useState(initialValue);
  return <DirectoryPicker value={value} onChange={setValue} />;
}

function makeEntries(names: string[], parentDir: string): DirEntry[] {
  return names.map((name) => ({ name, path: `${parentDir}${name}` }));
}

// Flush all fake timers AND pending microtasks (Promise callbacks) in one go.
async function flushAll() {
  await act(async () => {
    await vi.runAllTimersAsync();
  });
}

describe("DirectoryPicker", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  // ---------------------------------------------------------------------------
  // IPC call / cache behaviour
  // ---------------------------------------------------------------------------

  describe("IPC call behaviour", () => {
    it("calls listDirectory after the debounce when a value is set", async () => {
      // Given
      vi.mocked(commands.listDirectory).mockResolvedValue([]);
      render(<DirectoryPicker value="/Users/" onChange={vi.fn()} />);

      // When: debounce fires
      await flushAll();

      // Then
      expect(commands.listDirectory).toHaveBeenCalledWith("/Users/");
    });

    it("does NOT call listDirectory when value is empty", async () => {
      // Given / When
      render(<DirectoryPicker value="" onChange={vi.fn()} />);
      await flushAll();

      // Then
      expect(commands.listDirectory).not.toHaveBeenCalled();
    });

    it("makes NO additional IPC call when the user types multiple characters within the same parent directory", async () => {
      // Given: initial fetch for /Users/takumi/ fills the cache
      const entries = makeEntries(["apps", "projects"], "/Users/takumi/");
      vi.mocked(commands.listDirectory).mockResolvedValue(entries);
      render(<Controlled initialValue="/Users/takumi/" />);
      await flushAll();
      expect(commands.listDirectory).toHaveBeenCalledTimes(1);

      const input = screen.getByRole("combobox");

      // When: two more keystrokes, all within /Users/takumi/
      fireEvent.change(input, { target: { value: "/Users/takumi/a" } });
      await flushAll();

      fireEvent.change(input, { target: { value: "/Users/takumi/ap" } });
      await flushAll();

      // Then: still only one IPC call total — cache was preserved across all keystrokes
      expect(commands.listDirectory).toHaveBeenCalledTimes(1);
    });

    it("makes a NEW IPC call when the user navigates to a different parent directory", async () => {
      // Given: cache populated for /Users/takumi/
      const first = makeEntries(["apps", "projects"], "/Users/takumi/");
      const second = makeEntries(["cruise"], "/Users/takumi/apps/");
      vi.mocked(commands.listDirectory)
        .mockResolvedValueOnce(first)
        .mockResolvedValueOnce(second);

      const { rerender } = render(
        <DirectoryPicker value="/Users/takumi/" onChange={vi.fn()} />
      );
      await flushAll();
      expect(commands.listDirectory).toHaveBeenCalledTimes(1);

      // When: value moves to a new parent directory
      rerender(<DirectoryPicker value="/Users/takumi/apps/" onChange={vi.fn()} />);
      await flushAll();

      // Then: a second IPC call for the new directory
      expect(commands.listDirectory).toHaveBeenCalledTimes(2);
      expect(commands.listDirectory).toHaveBeenNthCalledWith(2, "/Users/takumi/apps/");
    });
  });

  // ---------------------------------------------------------------------------
  // Dropdown visibility and entry filtering
  // ---------------------------------------------------------------------------

  describe("dropdown visibility", () => {
    it("opens the listbox when IPC returns entries", async () => {
      // Given
      const entries = makeEntries(["takumi", "shared"], "/Users/");
      vi.mocked(commands.listDirectory).mockResolvedValue(entries);
      render(<DirectoryPicker value="/Users/" onChange={vi.fn()} />);

      // When
      await flushAll();

      // Then
      expect(screen.getByRole("listbox")).toBeInTheDocument();
      expect(screen.getByText("takumi/")).toBeInTheDocument();
      expect(screen.getByText("shared/")).toBeInTheDocument();
    });

    it("filters displayed entries by the typed prefix using the cache", async () => {
      // Given: cache populated with apps, projects, Documents
      const entries = makeEntries(["apps", "projects", "Documents"], "/Users/takumi/");
      vi.mocked(commands.listDirectory).mockResolvedValue(entries);
      const { rerender } = render(
        <DirectoryPicker value="/Users/takumi/" onChange={vi.fn()} />
      );
      await flushAll();
      expect(screen.getByRole("listbox")).toBeInTheDocument();

      // When: value changes to "/Users/takumi/a" — same parent dir, prefix "a"
      rerender(<DirectoryPicker value="/Users/takumi/a" onChange={vi.fn()} />);
      await flushAll();

      // Then: only "apps/" is visible (prefix filter applied from cache)
      expect(screen.getByText("apps/")).toBeInTheDocument();
      expect(screen.queryByText("projects/")).not.toBeInTheDocument();
      expect(screen.queryByText("Documents/")).not.toBeInTheDocument();
    });

    it("closes the listbox when IPC returns an empty array", async () => {
      // Given
      vi.mocked(commands.listDirectory).mockResolvedValue([]);
      render(<DirectoryPicker value="/nonexistent/" onChange={vi.fn()} />);

      // When
      await flushAll();

      // Then
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    it("closes the listbox when IPC throws an error", async () => {
      // Given
      vi.mocked(commands.listDirectory).mockRejectedValue(new Error("permission denied"));
      render(<DirectoryPicker value="/root/" onChange={vi.fn()} />);

      // When
      await flushAll();

      // Then
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });
  });

  // ---------------------------------------------------------------------------
  // Keyboard navigation
  // ---------------------------------------------------------------------------

  describe("keyboard navigation", () => {
    async function renderOpenDropdown() {
      const entries = makeEntries(["apps", "projects", "Documents"], "/Users/takumi/");
      vi.mocked(commands.listDirectory).mockResolvedValue(entries);
      const onChange = vi.fn();
      render(<DirectoryPicker value="/Users/takumi/" onChange={onChange} />);
      await flushAll();
      expect(screen.getByRole("listbox")).toBeInTheDocument();
      return { onChange };
    }

    it("closes the listbox on Escape", async () => {
      // Given: listbox is open
      await renderOpenDropdown();

      // When
      fireEvent.keyDown(screen.getByRole("combobox"), { key: "Escape" });

      // Then
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    it("highlights the first option on ArrowDown", async () => {
      // Given: listbox is open, nothing highlighted (highlighted = -1)
      await renderOpenDropdown();

      // When
      fireEvent.keyDown(screen.getByRole("combobox"), { key: "ArrowDown" });

      // Then: first option is aria-selected
      const options = screen.getAllByRole("option");
      expect(options[0]).toHaveAttribute("aria-selected", "true");
      expect(options[1]).toHaveAttribute("aria-selected", "false");
    });

    it("moves the highlight back up on ArrowUp", async () => {
      // Given: second item is highlighted
      await renderOpenDropdown();
      const input = screen.getByRole("combobox");
      fireEvent.keyDown(input, { key: "ArrowDown" });
      fireEvent.keyDown(input, { key: "ArrowDown" });

      // When
      fireEvent.keyDown(input, { key: "ArrowUp" });

      // Then: back to first item
      const options = screen.getAllByRole("option");
      expect(options[0]).toHaveAttribute("aria-selected", "true");
      expect(options[1]).toHaveAttribute("aria-selected", "false");
    });

    it("calls onChange with the selected path and closes the listbox on Enter", async () => {
      // Given: first item is highlighted
      const { onChange } = await renderOpenDropdown();
      fireEvent.keyDown(screen.getByRole("combobox"), { key: "ArrowDown" });

      // When
      fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" });

      // Then
      expect(onChange).toHaveBeenCalledWith("/Users/takumi/apps/");
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    it("does not call onChange or close the listbox when Enter is pressed with no highlight", async () => {
      // Given: listbox is open, nothing highlighted
      const { onChange } = await renderOpenDropdown();

      // When: Enter pressed without selecting anything
      fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" });

      // Then: no selection
      expect(onChange).not.toHaveBeenCalled();
      expect(screen.getByRole("listbox")).toBeInTheDocument();
    });
  });

  // ---------------------------------------------------------------------------
  // Cache reset after entry selection (selectEntry)
  // ---------------------------------------------------------------------------

  describe("cache reset after selectEntry", () => {
    it("calls onChange with path + '/' and closes the listbox when an entry is clicked", async () => {
      // Given
      vi.mocked(commands.listDirectory).mockResolvedValue(
        makeEntries(["apps"], "/Users/takumi/")
      );
      const onChange = vi.fn();
      render(<DirectoryPicker value="/Users/takumi/" onChange={onChange} />);
      await flushAll();
      expect(screen.getByRole("listbox")).toBeInTheDocument();

      // When: user clicks the "apps/" option
      fireEvent.mouseDown(screen.getByText("apps/"));

      // Then
      expect(onChange).toHaveBeenCalledWith("/Users/takumi/apps/");
      expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    });

    it("resets the cache after selection so the new directory is fetched on the next debounce", async () => {
      // Given: cache warm for /Users/takumi/ with the Controlled wrapper
      const firstEntries = makeEntries(["apps"], "/Users/takumi/");
      vi.mocked(commands.listDirectory).mockResolvedValue(firstEntries);

      render(<Controlled initialValue="/Users/takumi/" />);
      await flushAll();
      expect(commands.listDirectory).toHaveBeenCalledTimes(1);
      expect(screen.getByRole("listbox")).toBeInTheDocument();

      // When: user selects "apps/" (triggers selectEntry → cache reset → onChange("/Users/takumi/apps/"))
      vi.mocked(commands.listDirectory).mockResolvedValue(
        makeEntries(["cruise"], "/Users/takumi/apps/")
      );
      fireEvent.mouseDown(screen.getByText("apps/"));
      // The Controlled wrapper updates value to "/Users/takumi/apps/" via onChange
      await flushAll();

      // Then: listDirectory was called for the new directory
      // (cache was reset by selectEntry, so the new dir triggers a fresh IPC call)
      expect(commands.listDirectory).toHaveBeenCalledWith("/Users/takumi/apps/");
    });
  });

  // ---------------------------------------------------------------------------
  // Disabled state
  // ---------------------------------------------------------------------------

  describe("disabled state", () => {
    it("disables both the input and the Browse button when disabled=true", () => {
      // Given / When
      render(<DirectoryPicker value="" onChange={vi.fn()} disabled={true} />);

      // Then
      expect(screen.getByRole("combobox")).toBeDisabled();
      expect(screen.getByRole("button", { name: /browse/i })).toBeDisabled();
    });

    it("leaves input and Browse button enabled by default", () => {
      // Given / When
      render(<DirectoryPicker value="" onChange={vi.fn()} />);

      // Then
      expect(screen.getByRole("combobox")).not.toBeDisabled();
      expect(screen.getByRole("button", { name: /browse/i })).not.toBeDisabled();
    });
  });

  // ---------------------------------------------------------------------------
  // ARIA attributes
  // ---------------------------------------------------------------------------

  describe("ARIA attributes", () => {
    it("sets aria-expanded=false when the listbox is closed", () => {
      // Given / When
      render(<DirectoryPicker value="" onChange={vi.fn()} />);

      // Then
      expect(screen.getByRole("combobox")).toHaveAttribute("aria-expanded", "false");
    });

    it("sets aria-expanded=true when the listbox is open", async () => {
      // Given
      vi.mocked(commands.listDirectory).mockResolvedValue(
        makeEntries(["takumi"], "/Users/")
      );
      render(<DirectoryPicker value="/Users/" onChange={vi.fn()} />);

      // When
      await flushAll();

      // Then
      expect(screen.getByRole("combobox")).toHaveAttribute("aria-expanded", "true");
    });
  });
});
