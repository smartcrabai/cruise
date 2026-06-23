import { useEffect, useId, useRef, useState } from "react";
import { listGithubRepos } from "../lib/commands";

interface RepoPickerProps {
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
  placeholder?: string;
  id?: string;
}

function filterRepos(all: string[], query: string): string[] {
  const q = query.trim().toLowerCase();
  if (!q) return all;
  return all.filter((repo) => repo.toLowerCase().includes(q));
}

/**
 * Combobox for picking a GitHub repository (`owner/repo`).
 *
 * Suggestions come from `gh repo list` (loaded once on mount); free-form
 * input is always accepted so repositories outside the authenticated user's
 * account can be used too.
 */
export function RepoPicker({
  value,
  onChange,
  disabled = false,
  placeholder,
  id,
}: RepoPickerProps) {
  const [repos, setRepos] = useState<string[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [isOpen, setIsOpen] = useState(false);
  const [highlighted, setHighlighted] = useState<number>(-1);

  const uid = useId();
  const listboxId = `${uid}-listbox`;
  const containerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    let active = true;
    listGithubRepos()
      .then((result) => {
        if (active) {
          setRepos(result);
          setLoadError(null);
        }
      })
      .catch((e) => {
        if (active) setLoadError(String(e));
      });
    return () => {
      active = false;
    };
  }, []);

  // Close dropdown when clicking outside
  useEffect(() => {
    function handleMouseDown(e: MouseEvent) {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setIsOpen(false);
      }
    }
    document.addEventListener("mousedown", handleMouseDown);
    return () => document.removeEventListener("mousedown", handleMouseDown);
  }, []);

  const filtered = filterRepos(repos, value);

  function selectRepo(repo: string) {
    onChange(repo);
    setIsOpen(false);
    setHighlighted(-1);
    inputRef.current?.focus();
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (!isOpen) return;

    if (e.key === "ArrowDown") {
      e.preventDefault();
      setHighlighted((h) => Math.min(h + 1, filtered.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setHighlighted((h) => Math.max(h - 1, 0));
    } else if (e.key === "Enter") {
      if (highlighted >= 0 && highlighted < filtered.length) {
        e.preventDefault();
        selectRepo(filtered[highlighted]);
      }
    } else if (e.key === "Escape") {
      setIsOpen(false);
      setHighlighted(-1);
    }
  }

  return (
    <div ref={containerRef} className="relative">
      <input
        ref={inputRef}
        id={id}
        type="text"
        role="combobox"
        aria-expanded={isOpen}
        aria-autocomplete="list"
        aria-controls={listboxId}
        aria-activedescendant={highlighted >= 0 ? `${listboxId}-opt-${highlighted}` : undefined}
        value={value}
        onChange={(e) => {
          onChange(e.target.value);
          setIsOpen(true);
          setHighlighted(-1);
        }}
        onFocus={() => {
          if (filtered.length > 0) setIsOpen(true);
        }}
        onKeyDown={handleKeyDown}
        disabled={disabled}
        placeholder={placeholder ?? "owner/repository"}
        className="w-full bg-gray-50 dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm text-gray-800 dark:text-gray-200 placeholder-gray-400 dark:placeholder-gray-600 focus:border-blue-500 outline-none disabled:opacity-50"
      />

      {loadError && (
        <p className="mt-1 text-xs text-yellow-700 dark:text-yellow-400">
          Could not list repositories via gh ({loadError}); type owner/repository manually.
        </p>
      )}

      {isOpen && filtered.length > 0 && (
        <ul
          id={listboxId}
          role="listbox"
          className="absolute z-50 top-full left-0 right-0 mt-1 bg-gray-100 dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded shadow-lg max-h-56 overflow-auto"
        >
          {filtered.map((repo, i) => (
            <li
              key={repo}
              id={`${listboxId}-opt-${i}`}
              role="option"
              aria-selected={i === highlighted}
              onMouseDown={(e) => {
                e.preventDefault();
                selectRepo(repo);
              }}
              onMouseEnter={() => setHighlighted(i)}
              className={`px-3 py-1.5 text-sm text-gray-800 dark:text-gray-200 cursor-pointer ${
                i === highlighted ? "bg-gray-200 dark:bg-gray-700" : "hover:bg-gray-200 dark:hover:bg-gray-800"
              }`}
            >
              {repo}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
