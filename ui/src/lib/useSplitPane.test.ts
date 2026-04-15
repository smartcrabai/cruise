import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useSplitPane } from "./useSplitPane";

const STORAGE_KEY = "test-sidebar-width";
const DEFAULT_WIDTH = 288;
const MIN_WIDTH = 180;
const MAX_WIDTH = 480;

describe("useSplitPane", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe("initial width", () => {
    it("returns default width when localStorage is empty", () => {
      const { result } = renderHook(() =>
        useSplitPane(STORAGE_KEY, DEFAULT_WIDTH, MIN_WIDTH, MAX_WIDTH),
      );
      expect(result.current.width).toBe(DEFAULT_WIDTH);
    });

    it("loads width from localStorage when available", () => {
      localStorage.setItem(STORAGE_KEY, "350");
      const { result } = renderHook(() =>
        useSplitPane(STORAGE_KEY, DEFAULT_WIDTH, MIN_WIDTH, MAX_WIDTH),
      );
      expect(result.current.width).toBe(350);
    });

    it("clamps width to maxWidth when localStorage value exceeds maximum", () => {
      localStorage.setItem(STORAGE_KEY, "600");
      const { result } = renderHook(() =>
        useSplitPane(STORAGE_KEY, DEFAULT_WIDTH, MIN_WIDTH, MAX_WIDTH),
      );
      expect(result.current.width).toBe(MAX_WIDTH);
    });

    it("clamps width to minWidth when localStorage value is below minimum", () => {
      localStorage.setItem(STORAGE_KEY, "100");
      const { result } = renderHook(() =>
        useSplitPane(STORAGE_KEY, DEFAULT_WIDTH, MIN_WIDTH, MAX_WIDTH),
      );
      expect(result.current.width).toBe(MIN_WIDTH);
    });

    it("returns default when localStorage value is NaN", () => {
      localStorage.setItem(STORAGE_KEY, "not-a-number");
      const { result } = renderHook(() =>
        useSplitPane(STORAGE_KEY, DEFAULT_WIDTH, MIN_WIDTH, MAX_WIDTH),
      );
      expect(result.current.width).toBe(DEFAULT_WIDTH);
    });
  });

  describe("handleMouseDown", () => {
    it("provides a handleMouseDown callback", () => {
      const { result } = renderHook(() =>
        useSplitPane(STORAGE_KEY, DEFAULT_WIDTH, MIN_WIDTH, MAX_WIDTH),
      );
      expect(typeof result.current.handleMouseDown).toBe("function");
    });

    it("sets cursor to col-resize on document during drag", () => {
      const { result } = renderHook(() =>
        useSplitPane(STORAGE_KEY, DEFAULT_WIDTH, MIN_WIDTH, MAX_WIDTH),
      );

      act(() => {
        result.current.handleMouseDown({ clientX: 100, preventDefault: vi.fn() } as unknown as React.MouseEvent);
      });

      expect(document.body.style.cursor).toBe("col-resize");
      expect(document.body.style.userSelect).toBe("none");
    });
  });

  describe("drag behavior", () => {
    it("updates width on mousemove during drag", () => {
      const { result } = renderHook(() =>
        useSplitPane(STORAGE_KEY, DEFAULT_WIDTH, MIN_WIDTH, MAX_WIDTH),
      );

      act(() => {
        result.current.handleMouseDown({ clientX: 100, preventDefault: vi.fn() } as unknown as React.MouseEvent);
      });

      act(() => {
        const event = new MouseEvent("mousemove", { clientX: 200 });
        document.dispatchEvent(event);
      });

      expect(result.current.width).toBe(DEFAULT_WIDTH + 100);
    });

    it("clamps width to maxWidth during drag", () => {
      const { result } = renderHook(() =>
        useSplitPane(STORAGE_KEY, DEFAULT_WIDTH, MIN_WIDTH, MAX_WIDTH),
      );

      act(() => {
        result.current.handleMouseDown({ clientX: 100, preventDefault: vi.fn() } as unknown as React.MouseEvent);
      });

      act(() => {
        const event = new MouseEvent("mousemove", { clientX: 1000 });
        document.dispatchEvent(event);
      });

      expect(result.current.width).toBe(MAX_WIDTH);
    });

    it("clamps width to minWidth during drag", () => {
      const { result } = renderHook(() =>
        useSplitPane(STORAGE_KEY, DEFAULT_WIDTH, MIN_WIDTH, MAX_WIDTH),
      );

      act(() => {
        result.current.handleMouseDown({ clientX: 300, preventDefault: vi.fn() } as unknown as React.MouseEvent);
      });

      act(() => {
        const event = new MouseEvent("mousemove", { clientX: 0 });
        document.dispatchEvent(event);
      });

      expect(result.current.width).toBe(MIN_WIDTH);
    });

    it("ends drag on mouseup", () => {
      const { result } = renderHook(() =>
        useSplitPane(STORAGE_KEY, DEFAULT_WIDTH, MIN_WIDTH, MAX_WIDTH),
      );

      act(() => {
        result.current.handleMouseDown({ clientX: 100, preventDefault: vi.fn() } as unknown as React.MouseEvent);
      });

      act(() => {
        const event = new MouseEvent("mouseup");
        document.dispatchEvent(event);
      });

      expect(document.body.style.cursor).toBe("");
      expect(document.body.style.userSelect).toBe("");
    });

    it("allows new drag after mouseup", () => {
      const { result } = renderHook(() =>
        useSplitPane(STORAGE_KEY, DEFAULT_WIDTH, MIN_WIDTH, MAX_WIDTH),
      );

      act(() => {
        result.current.handleMouseDown({ clientX: 100, preventDefault: vi.fn() } as unknown as React.MouseEvent);
      });

      act(() => {
        const event = new MouseEvent("mouseup");
        document.dispatchEvent(event);
      });

      act(() => {
        result.current.handleMouseDown({ clientX: 200, preventDefault: vi.fn() } as unknown as React.MouseEvent);
      });

      act(() => {
        const event = new MouseEvent("mousemove", { clientX: 250 });
        document.dispatchEvent(event);
      });

      expect(result.current.width).toBe(DEFAULT_WIDTH + 50);
    });
  });

  describe("localStorage persistence", () => {
    it("persists width to localStorage on change", async () => {
      const { result } = renderHook(() =>
        useSplitPane(STORAGE_KEY, DEFAULT_WIDTH, MIN_WIDTH, MAX_WIDTH),
      );

      act(() => {
        result.current.handleMouseDown({ clientX: 100, preventDefault: vi.fn() } as unknown as React.MouseEvent);
      });

      act(() => {
        const event = new MouseEvent("mousemove", { clientX: 200 });
        document.dispatchEvent(event);
      });

      act(() => {
        const event = new MouseEvent("mouseup");
        document.dispatchEvent(event);
      });

      expect(localStorage.getItem(STORAGE_KEY)).toBe(String(result.current.width));
    });
  });

  describe("cleanup", () => {
    it("removes event listeners on unmount", () => {
      const removeEventListenerSpy = vi.spyOn(document, "removeEventListener");
      const { unmount } = renderHook(() =>
        useSplitPane(STORAGE_KEY, DEFAULT_WIDTH, MIN_WIDTH, MAX_WIDTH),
      );

      unmount();

      expect(removeEventListenerSpy).toHaveBeenCalledWith("mousemove", expect.any(Function));
      expect(removeEventListenerSpy).toHaveBeenCalledWith("mouseup", expect.any(Function));
    });

    it("does not crash when unmounting during drag", () => {
      const { result, unmount } = renderHook(() =>
        useSplitPane(STORAGE_KEY, DEFAULT_WIDTH, MIN_WIDTH, MAX_WIDTH),
      );

      act(() => {
        result.current.handleMouseDown({ clientX: 100, preventDefault: vi.fn() } as unknown as React.MouseEvent);
      });

      expect(() => unmount()).not.toThrow();
    });
  });
});
