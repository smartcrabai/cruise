import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";

interface PublishIssueDialogProps {
  sessionId: string;
  onConfirm: (mentionCruise: boolean) => Promise<void>;
  onCancel: () => void;
}

const FOCUSABLE = 'button:not([disabled]),[href],input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex="-1"])';

export function PublishIssueDialog({ sessionId, onConfirm, onCancel }: PublishIssueDialogProps) {
  const titleId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const [mentionCruise, setMentionCruise] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");
  const onCancelRef = useRef(onCancel);
  useLayoutEffect(() => { onCancelRef.current = onCancel; });

  useEffect(() => {
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !submitting) {
        e.preventDefault();
        onCancelRef.current();
      }
    };
    document.addEventListener("keydown", handleKey);
    return () => document.removeEventListener("keydown", handleKey);
  }, [submitting]);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;

    const focusable = () => Array.from(dialog.querySelectorAll<HTMLElement>(FOCUSABLE));
    focusable()[0]?.focus();

    const handleTab = (e: KeyboardEvent) => {
      if (e.key !== "Tab") return;
      const els = focusable();
      if (els.length === 0) return;
      const first = els[0];
      const last = els[els.length - 1];
      if (e.shiftKey) {
        if (document.activeElement === first) { e.preventDefault(); last.focus(); }
      } else {
        if (document.activeElement === last) { e.preventDefault(); first.focus(); }
      }
    };

    dialog.addEventListener("keydown", handleTab);
    return () => dialog.removeEventListener("keydown", handleTab);
  }, []);

  async function handleConfirm() {
    setSubmitting(true);
    setError("");
    try {
      await onConfirm(mentionCruise);
    } catch (e) {
      setError(String(e));
      setSubmitting(false);
    }
  }

  return (
    <div
      className="fixed inset-0 bg-black/60 flex items-center justify-center z-50"
      onClick={() => !submitting && onCancel()}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="bg-white dark:bg-gray-900 rounded-lg shadow-xl border border-gray-200 dark:border-gray-700 p-6 max-w-sm w-full space-y-4"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 id={titleId} className="text-lg font-semibold text-gray-900 dark:text-gray-100">Publish as GitHub Issue</h2>
        <p className="text-sm text-gray-500 dark:text-gray-400">
          Publish session {sessionId}&apos;s plan as a GitHub issue and delete the local session. This cannot be undone.
        </p>
        <label className="flex items-center gap-2 text-sm text-gray-700 dark:text-gray-300">
          <input
            type="checkbox"
            checked={mentionCruise}
            onChange={(e) => setMentionCruise(e.target.checked)}
            disabled={submitting}
          />
          Mention @cruise in the issue body
        </label>
        {error && <p className="text-sm text-red-600 dark:text-red-400">{error}</p>}
        <div className="flex gap-2 justify-end">
          <button
            type="button"
            onClick={onCancel}
            disabled={submitting}
            className="px-4 py-2 border border-gray-300 dark:border-gray-700 text-gray-500 dark:text-gray-400 rounded text-sm hover:bg-gray-200 dark:hover:bg-gray-800 disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={() => void handleConfirm()}
            disabled={submitting}
            className="px-4 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700 disabled:opacity-50"
          >
            {submitting ? "Publishing..." : "Publish"}
          </button>
        </div>
      </div>
    </div>
  );
}
