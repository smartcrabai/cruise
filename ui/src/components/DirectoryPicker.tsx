import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { listDirectory } from "../lib/commands";
import type { DirEntry } from "../types";

interface DirectoryPickerProps {
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  placeholder?: string;
}

/** Split a typed path into the parent dir and the incomplete last segment.
 *
 * Examples:
 *   "/Users/takumi/ap"  → { dir: "/Users/takumi/", prefix: "ap" }
 *   "/Users/takumi/"    → { dir: "/Users/takumi/", prefix: "" }
 *   "~/pr"              → { dir: "~/", prefix: "pr" }
 */
function splitPath(value: string): { dir: string; prefix: string } {
  const lastSlash = value.lastIndexOf("/");
  if (lastSlash === -1) {
    return { dir: "", prefix: value };
  }
  return {
    dir: value.slice(0, lastSlash + 1),
    prefix: value.slice(lastSlash + 1),
  };
}

function filterByPrefix(all: DirEntry[], prefix: string): DirEntry[] {
  if (!prefix) return all;
  const lower = prefix.toLowerCase();
  return all.filter((e) => e.name.toLowerCase().startsWith(lower));
}

export function DirectoryPicker({
  value,
  onChange,
  disabled = false,
  placeholder,
}: DirectoryPickerProps) {
  const [entries, setEntries] = useState<DirEntry[]>([]);
  const [isOpen, setIsOpen] = useState(false);
  const [highlighted, setHighlighted] = useState<number>(-1);
  const [cachedDir, setCachedDir] = useState<string | null>(null);
  const [cachedEntries, setCachedEntries] = useState<DirEntry[]>([]);

  const containerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const fetchEntries = useCallback(
    (inputValue: string) => {
      const { dir, prefix } = splitPath(inputValue);
      const queryPath = dir || inputValue;

      if (queryPath === cachedDir) {
        const filtered = filterByPrefix(cachedEntries, prefix);
        setEntries(filtered);
        setIsOpen(filtered.length > 0);
        return;
      }

      listDirectory(queryPath)
        .then((result) => {
          setCachedDir(queryPath);
          setCachedEntries(result);
          const filtered = filterByPrefix(result, prefix);
          setEntries(filtered);
          setHighlighted(-1);
          setIsOpen(filtered.length > 0);
        })
        .catch(() => {
          setEntries([]);
          setIsOpen(false);
        });
    },
    [cachedDir, cachedEntries]
  );

  // Debounced fetch on value change
  useEffect(() => {
    if (debounceRef.current !== null) {
      clearTimeout(debounceRef.current);
    }
    debounceRef.current = setTimeout(() => {
      if (value.length > 0) {
        fetchEntries(value);
      } else {
        setIsOpen(false);
      }
    }, 150);

    return () => {
      if (debounceRef.current !== null) {
        clearTimeout(debounceRef.current);
      }
    };
  }, [value, fetchEntries]);

  // Close dropdown when clicking outside
  useEffect(() => {
    function handleMouseDown(e: MouseEvent) {
      if (
        containerRef.current &&
        !containerRef.current.contains(e.target as Node)
      ) {
        setIsOpen(false);
      }
    }
    document.addEventListener("mousedown", handleMouseDown);
    return () => document.removeEventListener("mousedown", handleMouseDown);
  }, []);

  function selectEntry(entry: DirEntry) {
    const newValue = entry.path.endsWith("/")
      ? entry.path
      : entry.path + "/";
    onChange(newValue);
    setIsOpen(false);
    setHighlighted(-1);
    setCachedDir(null);
    inputRef.current?.focus();
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (!isOpen) return;

    if (e.key === "ArrowDown") {
      e.preventDefault();
      setHighlighted((h) => Math.min(h + 1, entries.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setHighlighted((h) => Math.max(h - 1, 0));
    } else if (e.key === "Enter") {
      if (highlighted >= 0 && highlighted < entries.length) {
        e.preventDefault();
        selectEntry(entries[highlighted]);
      }
    } else if (e.key === "Escape") {
      setIsOpen(false);
      setHighlighted(-1);
    }
  }

  async function handleBrowse() {
    try {
      const selected = await open({ directory: true, multiple: false });
      if (typeof selected === "string") {
        onChange(selected.endsWith("/") ? selected : selected + "/");
        setCachedDir(null);
      }
    } catch {
      // user cancelled or dialog failed — ignore
    }
  }

  return (
    <div ref={containerRef} className="relative">
      <div className="flex gap-2">
        <input
          ref={inputRef}
          type="text"
          value={value}
          onChange={(e) => {
            onChange(e.target.value);
            setCachedDir(null);
          }}
          onFocus={() => {
            if (value.length > 0) fetchEntries(value);
          }}
          onKeyDown={handleKeyDown}
          disabled={disabled}
          placeholder={placeholder}
          className="flex-1 bg-gray-900 border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 placeholder-gray-600 focus:border-blue-500 outline-none disabled:opacity-50"
        />
        <button
          type="button"
          onClick={() => void handleBrowse()}
          disabled={disabled}
          className="px-3 py-2 bg-gray-800 border border-gray-700 rounded text-sm text-gray-300 hover:bg-gray-700 disabled:opacity-50 disabled:cursor-not-allowed whitespace-nowrap"
        >
          Browse
        </button>
      </div>

      {isOpen && entries.length > 0 && (
        <ul className="absolute z-50 top-full left-0 right-0 mt-1 bg-gray-800 border border-gray-700 rounded shadow-lg max-h-56 overflow-auto">
          {entries.map((entry, i) => (
            <li
              key={entry.path}
              onMouseDown={(e) => {
                e.preventDefault();
                selectEntry(entry);
              }}
              onMouseEnter={() => setHighlighted(i)}
              className={`px-3 py-1.5 text-sm text-gray-200 cursor-pointer ${
                i === highlighted ? "bg-gray-700" : "hover:bg-gray-800"
              }`}
            >
              {entry.name}/
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
