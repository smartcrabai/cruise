import { useCallback, useEffect, useRef } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "webp", "gif"] as const;

/** True when `path` ends in a recognized image extension (case-insensitive). */
function isImagePath(path: string): boolean {
  const lower = path.toLowerCase();
  return IMAGE_EXTENSIONS.some((ext) => lower.endsWith(`.${ext}`));
}

/** Strip duplicates while preserving order. */
function dedupe(paths: string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const p of paths) {
    if (!seen.has(p)) {
      seen.add(p);
      out.push(p);
    }
  }
  return out;
}

interface ImageAttachmentsProps {
  /** Absolute paths of attached image files. */
  value: string[];
  /** Called with the next list after add/remove. */
  onChange: (next: string[]) => void;
  /** True while a parent action (plan generation) is in flight; disables interactions. */
  disabled?: boolean;
}

/**
 * Image attachment area for the New Session form: a drop zone that listens to
 * Tauri's webview drag-drop events, a "Browse" button using the dialog plugin,
 * and per-item previews with a remove control.
 *
 * Only file extensions matching {@link IMAGE_EXTENSIONS} are accepted; other
 * dropped files are silently ignored so users can still drop non-images into
 * the underlying text area without surprise.
 */
export function ImageAttachments({ value, onChange, disabled }: ImageAttachmentsProps) {
  const valueRef = useRef(value);
  const onChangeRef = useRef(onChange);
  useEffect(() => {
    valueRef.current = value;
  }, [value]);
  useEffect(() => {
    onChangeRef.current = onChange;
  }, [onChange]);

  // Tauri swallows the webview's native HTML5 drop events when dragDrop is
  // enabled (the default), so we have to listen on the webview itself to get
  // absolute file paths. In non-Tauri contexts (vitest/jsdom) `getCurrentWebview`
  // throws or rejects because the IPC bridge isn't installed; treat that as
  // "no drag-drop available" so the form still renders.
  useEffect(() => {
    if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) {
      return;
    }
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    try {
      getCurrentWebview()
        .onDragDropEvent((event) => {
          if (event.payload.type !== "drop") return;
          const images = event.payload.paths.filter(isImagePath);
          if (images.length === 0) return;
          onChangeRef.current(dedupe([...valueRef.current, ...images]));
        })
        .then((u) => {
          if (cancelled) {
            u();
          } else {
            unlisten = u;
          }
        })
        .catch((e: unknown) => {
          console.warn("ImageAttachments: drag-drop subscription failed", e);
        });
    } catch (e) {
      // Tauri webview unavailable -- drop support stays off; the Browse button still works.
      console.warn("ImageAttachments: drag-drop unavailable", e);
    }
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const handleBrowse = useCallback(async () => {
    const selected = await openDialog({
      multiple: true,
      filters: [{ name: "Images", extensions: IMAGE_EXTENSIONS.slice() }],
    });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    onChangeRef.current(dedupe([...valueRef.current, ...paths.filter(isImagePath)]));
  }, []);

  function remove(path: string) {
    onChangeRef.current(value.filter((p) => p !== path));
  }

  function basename(path: string): string {
    const idx = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
    return idx >= 0 ? path.slice(idx + 1) : path;
  }

  return (
    <div className="space-y-1.5">
      <div className="flex items-center justify-between">
        <span className="text-xs text-gray-500 dark:text-gray-400 uppercase tracking-wide">Images</span>
        <button
          type="button"
          onClick={() => void handleBrowse()}
          disabled={disabled}
          className="text-xs text-blue-600 dark:text-blue-400 hover:text-blue-500 dark:hover:text-blue-300 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          Browse…
        </button>
      </div>
      {value.length === 0 ? (
        <div className="text-xs text-gray-500 dark:text-gray-400 border border-dashed border-gray-300 dark:border-gray-700 rounded px-3 py-3 text-center">
          Drop image files here, or click Browse…
        </div>
      ) : (
        <ul className="flex flex-wrap gap-2">
          {value.map((path) => (
            <li
              key={path}
              className="relative group border border-gray-300 dark:border-gray-700 rounded overflow-hidden bg-gray-50 dark:bg-gray-900"
              title={path}
            >
              <img
                src={convertFileSrc(path)}
                alt={basename(path)}
                className="w-20 h-20 object-cover"
              />
              <button
                type="button"
                aria-label={`Remove ${basename(path)}`}
                onClick={() => remove(path)}
                disabled={disabled}
                className="absolute top-0.5 right-0.5 bg-black/70 text-white text-xs rounded w-5 h-5 flex items-center justify-center hover:bg-black disabled:opacity-50"
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
