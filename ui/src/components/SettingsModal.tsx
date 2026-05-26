import { useId, useState } from "react";
import type { AppConfig } from "../types";

export interface SettingsModalProps {
  initialParallelism: number;
  onSave: (config: AppConfig) => Promise<void>;
  onClose: () => void;
}

export function SettingsModal({ initialParallelism, onSave, onClose }: SettingsModalProps) {
  const [value, setValue] = useState(String(initialParallelism));
  const [error, setError] = useState("");
  const [isSaving, setIsSaving] = useState(false);
  const titleId = useId();

  const handleSave = async () => {
    const num = Number(value);
    if (!value.trim() || isNaN(num) || !Number.isInteger(num) || num < 1) {
      setError("Must be at least 1");
      return;
    }
    setError("");
    setIsSaving(true);
    try {
      await onSave({ runAllParallelism: num });
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <div className="fixed inset-0 flex items-center justify-center bg-black/60">
      <div role="dialog" aria-modal="true" aria-labelledby={titleId} className="bg-gray-900 border border-gray-700 rounded-lg p-6 max-w-sm w-full space-y-4">
        <h2 id={titleId} className="text-gray-100 font-semibold text-xl">Settings</h2>

        <div className="space-y-1.5">
          <label htmlFor="run-all-parallelism" className="text-sm text-gray-300">
            Run All Parallelism
          </label>
          <input
            id="run-all-parallelism"
            type="number"
            value={value}
            onChange={(e) => setValue(e.target.value)}
            disabled={isSaving}
            className="w-full bg-gray-800 border border-gray-700 rounded px-3 py-2 text-sm text-gray-200"
          />
          {error && <p className="text-sm text-red-400">{error}</p>}
        </div>

        <div className="flex gap-2">
          <button
            type="button"
            onClick={() => void handleSave()}
            disabled={isSaving}
            className="px-4 py-2 bg-blue-600 text-white rounded text-sm disabled:opacity-50"
          >
            {isSaving ? "Saving..." : "Save"}
          </button>
          <button
            type="button"
            onClick={onClose}
            disabled={isSaving}
            className="px-4 py-2 bg-gray-700 text-gray-300 rounded text-sm disabled:opacity-50"
          >
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
