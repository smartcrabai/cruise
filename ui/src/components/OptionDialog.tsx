import { useId, useState } from "react";
import type { ChoiceDto } from "../types";
import { MarkdownViewer } from "./MarkdownViewer";

export interface OptionDialogProps {
  choices: ChoiceDto[];
  plan?: string;
  onRespond: (result: { nextStep?: string; textInput?: string }) => void;
}

export function OptionDialog({ choices, plan, onRespond }: OptionDialogProps) {
  const [textInputValues, setTextInputValues] = useState<Record<string, string>>({});
  const titleId = useId();

  const handleTextInputSubmit = (index: number, choice: ChoiceDto) => {
    const key = String(index);
    const trimmed = (textInputValues[key] ?? "").trim();
    if (!trimmed) return;
    onRespond({ nextStep: choice.next, textInput: trimmed });
  };

  return (
    <div className="fixed inset-0 flex items-center justify-center bg-black/60">
      <div role="dialog" aria-modal="true" aria-labelledby={titleId} className="bg-gray-50 dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded-lg p-6 max-w-lg w-full space-y-4">
        <h2 id={titleId} className="text-gray-900 dark:text-gray-100 font-semibold text-lg">Choose an option</h2>

        {plan && (
          <div className="max-h-64 overflow-y-auto">
            <MarkdownViewer content={plan} />
          </div>
        )}

        <div className="space-y-3">
          {choices.map((choice, index) => {
            const key = String(index);
            if (choice.kind === "textInput") {
              const inputId = `option-input-${key}`;
              return (
                <div key={key} className="space-y-2">
                  <label htmlFor={inputId} className="text-sm text-gray-700 dark:text-gray-300 block">
                    {choice.label}
                  </label>
                  <input
                    id={inputId}
                    type="text"
                    value={textInputValues[key] ?? ""}
                    onChange={(e) =>
                      setTextInputValues((prev) => ({ ...prev, [key]: e.target.value }))
                    }
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        handleTextInputSubmit(index, choice);
                      }
                    }}
                    className="w-full bg-gray-100 dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm text-gray-800 dark:text-gray-200"
                  />
                  <button
                    type="button"
                    onClick={() => handleTextInputSubmit(index, choice)}
                    disabled={!(textInputValues[key] ?? "").trim()}
                    className="px-4 py-2 bg-blue-600 text-white rounded text-sm disabled:opacity-50"
                  >
                    Submit
                  </button>
                </div>
              );
            }
            return (
              <button
                key={key}
                type="button"
                onClick={() => onRespond({ nextStep: choice.next })}
                className="w-full px-4 py-2 bg-gray-200 dark:bg-gray-700 text-gray-800 dark:text-gray-200 rounded text-sm hover:bg-gray-300 dark:hover:bg-gray-600 text-left"
              >
                {choice.label}
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
}
