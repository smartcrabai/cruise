import { useEffect, useId, useState } from "react";
import { respondToAsk } from "../lib/commands";
import { ASK_USER_EVENT, type AskUserDetail } from "../lib/askUser";

/**
 * Modal that prompts the user to answer an SDK `ask_user` question raised during
 * plan generation. Mounted once at the app root; it listens for the
 * {@link ASK_USER_EVENT} window event (dispatched from every PlanEvent handler)
 * so a single dialog serves the create / regenerate / fix flows.
 */
export function AskUserDialog() {
  const [pending, setPending] = useState<AskUserDetail | null>(null);
  const [answer, setAnswer] = useState("");
  const titleId = useId();
  const questionId = useId();

  useEffect(() => {
    const handler = (e: Event) => {
      const detail = (e as CustomEvent<AskUserDetail>).detail;
      setPending(detail);
      setAnswer("");
    };
    window.addEventListener(ASK_USER_EVENT, handler as EventListener);
    return () => window.removeEventListener(ASK_USER_EVENT, handler as EventListener);
  }, []);

  if (!pending) return null;

  const submit = () => {
    respondToAsk(pending.sessionId, answer).catch((e) => {
      // The agent is blocked waiting on this answer; surface routing failures
      // (e.g. no pending dialog for this session) instead of silently closing.
      console.error("Failed to deliver ask_user answer:", e);
    });
    setPending(null);
    setAnswer("");
  };

  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50">
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="bg-gray-900 rounded-lg shadow-xl border border-gray-700 p-6 max-w-lg w-full space-y-4"
      >
        <h2 id={titleId} className="text-lg font-semibold text-gray-100">
          The planning agent has a question
        </h2>
        <p id={questionId} className="text-sm text-gray-300 whitespace-pre-wrap">
          {pending.question}
        </p>
        <textarea
          autoFocus
          aria-label="Your answer"
          aria-describedby={questionId}
          value={answer}
          onChange={(e) => setAnswer(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) submit();
          }}
          className="w-full h-28 border border-gray-700 bg-gray-800 rounded px-3 py-2 text-sm text-gray-200 placeholder-gray-600 outline-none focus:border-blue-500 resize-none"
          placeholder="Your answer… (⌘/Ctrl+Enter to send)"
        />
        <div className="flex justify-end">
          <button
            type="button"
            onClick={submit}
            className="px-4 py-1.5 bg-blue-600 text-white rounded text-sm hover:bg-blue-700"
          >
            Send answer
          </button>
        </div>
      </div>
    </div>
  );
}
