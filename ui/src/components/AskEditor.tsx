export interface AskEditorProps {
  question: string;
  onQuestionChange: (value: string) => void;
  phase: "idle" | "editing" | "submitting";
  error: string | null;
  onSubmit: () => void;
  onCancel: () => void;
  className?: string;
}

export function AskEditor({
  question,
  onQuestionChange,
  phase,
  error,
  onSubmit,
  onCancel,
  className,
}: AskEditorProps) {
  const isSubmitting = phase === "submitting";
  const isEmpty = !question.trim();

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      e.preventDefault();
      if (!isEmpty && !isSubmitting) {
        onSubmit();
      }
    }
  };

  return (
    <div className={className}>
      <label htmlFor="ask-question" className="sr-only">
        Ask a question about the plan
      </label>
      <textarea
        id="ask-question"
        placeholder="Ask a question about the plan..."
        value={question}
        onChange={(e) => onQuestionChange(e.target.value)}
        disabled={isSubmitting}
        onKeyDown={handleKeyDown}
        className="w-full bg-gray-100 dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm text-gray-800 dark:text-gray-200 resize-none"
        rows={4}
      />
      {error && <p className="text-sm text-red-600 dark:text-red-400 mt-1">{error}</p>}
      <div className="flex gap-2 mt-2">
        <button
          type="button"
          onClick={onSubmit}
          disabled={isEmpty || isSubmitting}
          className="px-4 py-2 bg-blue-600 text-white rounded text-sm disabled:opacity-50"
        >
          {isSubmitting ? "Asking..." : "Submit"}
        </button>
        <button
          type="button"
          onClick={onCancel}
          disabled={isSubmitting}
          className="px-4 py-2 bg-gray-200 dark:bg-gray-700 text-gray-700 dark:text-gray-300 rounded text-sm disabled:opacity-50"
        >
          Cancel
        </button>
      </div>
    </div>
  );
}
