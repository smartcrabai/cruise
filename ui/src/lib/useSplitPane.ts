import { useCallback, useEffect, useRef, useState } from "react";

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}

export function useSplitPane(
  storageKey: string,
  defaultWidth: number,
  minWidth: number,
  maxWidth: number,
) {
  const [width, setWidth] = useState<number>(() => {
    try {
      const saved = localStorage.getItem(storageKey);
      if (saved !== null) {
        const parsed = parseInt(saved, 10);
        if (!isNaN(parsed)) return clamp(parsed, minWidth, maxWidth);
      }
    } catch {
      // localStorage unavailable in restricted environments
    }
    return defaultWidth;
  });

  // Assign during render so stable callbacks always read the current value
  const widthRef = useRef(width);
  widthRef.current = width;

  const isResizingRef = useRef(false);
  const startXRef = useRef(0);
  const startWidthRef = useRef(0);
  const dragWidthRef = useRef(width);

  const handleMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    isResizingRef.current = true;
    startXRef.current = e.clientX;
    startWidthRef.current = widthRef.current;
    dragWidthRef.current = widthRef.current;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
  }, []);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      const direction = e.key === "ArrowLeft" ? -1 : e.key === "ArrowRight" ? 1 : 0;
      if (direction === 0) return;
      e.preventDefault();
      const step = (e.shiftKey ? 50 : 10) * direction;
      const next = clamp(widthRef.current + step, minWidth, maxWidth);
      setWidth(next);
      try {
        localStorage.setItem(storageKey, String(next));
      } catch {
        // localStorage unavailable in restricted environments
      }
    },
    [minWidth, maxWidth, storageKey],
  );

  useEffect(() => {
    const handleMouseMove = (e: MouseEvent) => {
      if (!isResizingRef.current) return;
      const delta = e.clientX - startXRef.current;
      const next = clamp(startWidthRef.current + delta, minWidth, maxWidth);
      if (next === dragWidthRef.current) return;
      dragWidthRef.current = next;
      setWidth(next);
    };
    const handleMouseUp = () => {
      if (!isResizingRef.current) return;
      isResizingRef.current = false;
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      try {
        localStorage.setItem(storageKey, String(dragWidthRef.current));
      } catch {
        // localStorage unavailable in restricted environments
      }
    };
    document.addEventListener("mousemove", handleMouseMove);
    document.addEventListener("mouseup", handleMouseUp);
    return () => {
      document.removeEventListener("mousemove", handleMouseMove);
      document.removeEventListener("mouseup", handleMouseUp);
      if (isResizingRef.current) {
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
        isResizingRef.current = false;
      }
    };
  }, [minWidth, maxWidth, storageKey]);

  return { width, handleMouseDown, handleKeyDown };
}
