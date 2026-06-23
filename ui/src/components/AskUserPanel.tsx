import { useId, useState } from "react";
import { respondToAsk } from "../lib/commands";

export interface AskUserPanelProps {
  sessionId: string;
  question: string;
  onAnswered?: () => void;
}

/** Inline session-detail answer form for SDK ask_user questions raised during planning. */
export function AskUserPanel({ sessionId, question, onAnswered }: AskUserPanelProps) {
  const [answer, setAnswer] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");
  const titleId = useId();
  const questionId = useId();

  const submit = async () => {
    setSubmitting(true);
    setError("");
    try {
      await respondToAsk(sessionId, answer);
      setAnswer("");
      onAnswered?.();
    } catch (e) {
      setError(String(e));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <section
      aria-labelledby={titleId}
      className="rounded border border-blue-300/60 dark:border-blue-800/60 bg-blue-50/50 dark:bg-blue-950/20 p-4 space-y-3"
    >
      <div>
        <h3 id={titleId} className="text-sm font-semibold text-blue-800 dark:text-blue-200">
          The planning agent has a question
        </h3>
        <p id={questionId} className="mt-1 text-sm text-gray-700 dark:text-gray-300 whitespace-pre-wrap">
          {question}
        </p>
      </div>
      <textarea
        autoFocus
        aria-label="Your answer"
        aria-describedby={questionId}
        value={answer}
        onChange={(e) => setAnswer(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void submit();
        }}
        className="w-full h-28 border border-gray-300 dark:border-gray-700 bg-gray-50 dark:bg-gray-900 rounded px-3 py-2 text-sm text-gray-800 dark:text-gray-200 placeholder-gray-400 dark:placeholder-gray-600 outline-none focus:border-blue-500 resize-none"
        placeholder="Your answer... (Cmd/Ctrl+Enter to send)"
        disabled={submitting}
      />
      {error && <p className="text-sm text-red-600 dark:text-red-400">{error}</p>}
      <div className="flex justify-end">
        <button
          type="button"
          onClick={() => void submit()}
          disabled={submitting}
          className="px-4 py-1.5 bg-blue-600 text-white rounded text-sm hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {submitting ? "Sending…" : "Send answer"}
        </button>
      </div>
    </section>
  );
}
