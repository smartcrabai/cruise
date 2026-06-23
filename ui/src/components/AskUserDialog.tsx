import { useEffect, useId, useRef, useState } from "react";
import { respondToAsk } from "../lib/commands";
import { ASK_USER_EVENT, type AskUserDetail } from "../lib/askUser";

interface AskUserDialogProps {
  /** Called with the session id after the user's answer is successfully submitted. */
  onAnswered?: (sessionId: string) => void;
}

type SubmitState =
  | { status: "idle" }
  | { status: "submitting" }
  | { status: "error"; message: string };

/**
 * Modal that prompts the user to answer an SDK `ask_user` question raised during
 * plan generation. Mounted once at the app root; it listens for the
 * {@link ASK_USER_EVENT} window event (dispatched from every PlanEvent handler)
 * so a single dialog serves the create / regenerate / fix flows.
 */
export function AskUserDialog({ onAnswered }: AskUserDialogProps) {
  const [pending, setPending] = useState<AskUserDetail | null>(null);
  const [answer, setAnswer] = useState("");
  const [submitState, setSubmitState] = useState<SubmitState>({ status: "idle" });
  const titleId = useId();
  const questionId = useId();
  // Monotonically-increasing nonce so consecutive ask_user calls within the
  // same session can be distinguished.  Without this, Q1's .then() would see
  // the same sessionId as Q2 and dismiss Q2's dialog.
  const askNonceRef = useRef(0);

  useEffect(() => {
    const handler = (e: Event) => {
      const detail = (e as CustomEvent<AskUserDetail>).detail;
      askNonceRef.current += 1;
      setPending(detail);
      setAnswer("");
      setSubmitState({ status: "idle" });
    };
    window.addEventListener(ASK_USER_EVENT, handler as EventListener);
    return () => window.removeEventListener(ASK_USER_EVENT, handler as EventListener);
  }, []);

  if (!pending) return null;

  const submitting = submitState.status === "submitting";

  const submit = () => {
    if (submitting) return;
    const { sessionId } = pending;
    const nonce = askNonceRef.current;
    setSubmitState({ status: "submitting" });
    respondToAsk(sessionId, answer)
      .then(() => {
        if (askNonceRef.current !== nonce) return;
        setPending(null);
        setAnswer("");
        onAnswered?.(sessionId);
      })
      .catch((e) => {
        if (askNonceRef.current !== nonce) return;
        // Keep the dialog open so the user can retry or see the failure reason.
        console.error("Failed to deliver ask_user answer:", e);
        setSubmitState({ status: "error", message: e instanceof Error ? e.message : String(e) });
      });
  };

  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50">
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="bg-gray-50 dark:bg-gray-900 rounded-lg shadow-xl border border-gray-300 dark:border-gray-700 p-6 max-w-lg w-full space-y-4"
      >
        <h2 id={titleId} className="text-lg font-semibold text-gray-900 dark:text-gray-100">
          The planning agent has a question
        </h2>
        <p id={questionId} className="text-sm text-gray-700 dark:text-gray-300 whitespace-pre-wrap">
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
          className="w-full h-28 border border-gray-300 dark:border-gray-700 bg-gray-100 dark:bg-gray-800 rounded px-3 py-2 text-sm text-gray-800 dark:text-gray-200 placeholder-gray-400 dark:placeholder-gray-600 outline-none focus:border-blue-500 resize-none"
          placeholder="Your answer... (Cmd/Ctrl+Enter to send)"
          disabled={submitting}
        />
        {submitState.status === "error" && (
          <p className="text-sm text-red-600 dark:text-red-400">{submitState.message}</p>
        )}
        <div className="flex justify-end">
          <button
            type="button"
            onClick={submit}
            disabled={submitting}
            className="px-4 py-1.5 bg-blue-600 text-white rounded text-sm hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {submitting ? "Sending…" : "Send answer"}
          </button>
        </div>
      </div>
    </div>
  );
}
