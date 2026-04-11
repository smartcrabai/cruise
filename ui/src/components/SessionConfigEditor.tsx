import { useCallback, useEffect, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import type { ConfigEntry, PlanEvent, SkippableStepDto } from "../types";
import { getNewSessionConfigDefaults, listConfigs, updateSessionSettings, regenerateSessionPlan } from "../lib/commands";
import { collectExpandedStepIds } from "../lib/stepUtils";
import { Spinner } from "./Spinner";

interface SessionConfigEditorProps {
  sessionId: string;
  baseDir: string;
  configPath?: string;
  skippedSteps: string[];
  onSessionUpdated: (session: import("../types").Session) => void;
  onPlanRegenerated: (content: string) => void;
  onRegeneratingChange?: (isRegenerating: boolean) => void;
  onError: (error: string) => void;
  disabled?: boolean;
}

export function SessionConfigEditor({
  sessionId,
  baseDir,
  configPath,
  skippedSteps,
  onSessionUpdated,
  onPlanRegenerated,
  onRegeneratingChange,
  onError,
  disabled = false,
}: SessionConfigEditorProps) {
  const [configs, setConfigs] = useState<ConfigEntry[]>([]);
  const [configSteps, setConfigSteps] = useState<SkippableStepDto[]>([]);
  const [selectedConfigPath, setSelectedConfigPath] = useState<string>(configPath ?? "");
  const [selectedSkippedSteps, setSelectedSkippedSteps] = useState<Set<string>>(new Set(skippedSteps));
  const [isSaving, setIsSaving] = useState(false);
  const [isRegenerating, setIsRegenerating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Ref to read the latest skippedSteps in the config-loading effect without
  // re-triggering it on every parent re-render (array reference changes after save).
  const skippedStepsRef = useRef(skippedSteps);
  skippedStepsRef.current = skippedSteps;

  const configLookupBaseDir = selectedConfigPath ? "." : baseDir;
  const isDisabled = disabled || isSaving || isRegenerating;

  useEffect(() => {
    let active = true;
    const currentConfig = configPath ?? "";
    void listConfigs()
      .then((c) => {
        if (active) {
          if (currentConfig && !c.some((cfg) => cfg.path === currentConfig)) {
            setConfigs([
              ...c,
              { name: currentConfig.split("/").pop() ?? currentConfig, path: currentConfig },
            ]);
          } else {
            setConfigs(c);
          }
        }
      })
      .catch((e: unknown) => {
        if (active) setError(String(e));
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    void getNewSessionConfigDefaults({
      baseDir: configLookupBaseDir,
      configPath: selectedConfigPath || undefined,
    })
      .then((defaults) => {
        if (active) {
          const validStepIds = collectExpandedStepIds(defaults.steps);
          setConfigSteps(defaults.steps);
          setSelectedSkippedSteps(
            new Set(skippedStepsRef.current.filter((id) => validStepIds.has(id))),
          );
        }
      })
      .catch((e: unknown) => {
        if (active) {
          console.error("Failed to load config defaults:", e);
          setConfigSteps([]);
          setSelectedSkippedSteps(new Set());
        }
      });
    return () => {
      active = false;
    };
  }, [configLookupBaseDir, selectedConfigPath]);

  const isParentChecked = useCallback(
    (node: SkippableStepDto): boolean => {
      return node.expandedStepIds.every((id) => selectedSkippedSteps.has(id));
    },
    [selectedSkippedSteps],
  );

  const isParentIndeterminate = useCallback(
    (node: SkippableStepDto): boolean => {
      const selected = node.expandedStepIds.filter((id) => selectedSkippedSteps.has(id));
      return selected.length > 0 && selected.length < node.expandedStepIds.length;
    },
    [selectedSkippedSteps],
  );

  const toggleStepIds = useCallback((ids: Iterable<string>, checked: boolean) => {
    setSelectedSkippedSteps((prev) => {
      const next = new Set(prev);
      for (const id of ids) {
        if (checked) {
          next.add(id);
        } else {
          next.delete(id);
        }
      }
      return next;
    });
  }, []);

  const stepNodeLabel = useCallback((node: SkippableStepDto, isChild: boolean): string => {
    if (!isChild) {
      return node.id;
    }
    const slash = node.id.lastIndexOf("/");
    return slash === -1 ? node.id : node.id.slice(slash + 1);
  }, []);

  const renderStepNode = useCallback(
    (node: SkippableStepDto, isChild: boolean): React.ReactElement => {
      const label = stepNodeLabel(node, isChild);

      if (node.children.length === 0) {
        return (
          <label
            key={node.id}
            className={`flex items-center gap-2 cursor-pointer${isChild ? " pl-6" : ""}`}
          >
            <input
              type="checkbox"
              checked={selectedSkippedSteps.has(node.id)}
              onChange={(e) => toggleStepIds([node.id], e.target.checked)}
              disabled={isDisabled}
              className="accent-blue-500"
            />
            <span className="text-sm text-gray-300">{label}</span>
          </label>
        );
      }
      return (
        <div key={node.id}>
          <label className="flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={isParentChecked(node)}
              ref={(el) => {
                if (el) el.indeterminate = isParentIndeterminate(node);
              }}
              onChange={(e) => toggleStepIds(node.expandedStepIds, e.target.checked)}
              disabled={isDisabled}
              className="accent-blue-500"
            />
            <span className="text-sm text-gray-300 font-medium">{label}</span>
          </label>
          <div className="space-y-1 ml-4">
            {node.children.map((child) => renderStepNode(child, true))}
          </div>
        </div>
      );
    },
    [isDisabled, isParentChecked, isParentIndeterminate, selectedSkippedSteps, stepNodeLabel, toggleStepIds],
  );

  const hasConfigChanged = selectedConfigPath !== (configPath ?? "");
  const hasSkipChanged =
    selectedSkippedSteps.size !== skippedSteps.length ||
    Array.from(selectedSkippedSteps).some((id) => !skippedSteps.includes(id));

  const buildSettings = () => ({
    configPath: selectedConfigPath || undefined,
    skippedSteps: Array.from(selectedSkippedSteps),
  });

  const handleSaveAndRegenerate = async () => {
    setError(null);
    setIsRegenerating(true);
    onRegeneratingChange?.(true);
    try {
      const updated = await updateSessionSettings(sessionId, buildSettings());
      onSessionUpdated(updated);

      const channel = new Channel<PlanEvent>();
      channel.onmessage = (event) => {
        if (event.event === "planGenerated") {
          onPlanRegenerated(event.data.content);
        }
      };
      await regenerateSessionPlan(sessionId, channel);
    } catch (e) {
      const msg = String(e);
      setError(msg);
      onError(msg);
    } finally {
      setIsRegenerating(false);
      onRegeneratingChange?.(false);
    }
  };

  const handleSkipOnlySave = async () => {
    setError(null);
    setIsSaving(true);
    try {
      const updated = await updateSessionSettings(sessionId, buildSettings());
      onSessionUpdated(updated);
    } catch (e) {
      const msg = String(e);
      setError(msg);
      onError(msg);
    } finally {
      setIsSaving(false);
    }
  };

  return (
    <div className="space-y-4">
      {error && (
        <div className="bg-red-900/40 border border-red-700 rounded px-4 py-3 text-sm text-red-300">
          {error}
        </div>
      )}

      <div className="space-y-1.5">
        <label
          htmlFor="session-config-select"
          className="text-xs text-gray-500 uppercase tracking-wide"
        >
          Config
        </label>
        <select
          id="session-config-select"
          value={selectedConfigPath}
          onChange={(e) => setSelectedConfigPath(e.target.value)}
          disabled={isDisabled}
          className="w-full bg-gray-900 border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:border-blue-500 outline-none disabled:opacity-50"
        >
          <option value="">Auto (repo / ~/.cruise / builtin)</option>
          {configs.map((c) => (
            <option key={c.path} value={c.path}>
              {c.name}
            </option>
          ))}
        </select>
      </div>

      {configSteps.length > 0 && (
        <div className="space-y-1.5">
          <label className="text-xs text-gray-500 uppercase tracking-wide">Skip Steps</label>
          <div className="space-y-1 max-h-40 overflow-y-auto">
            {configSteps.map((node) => renderStepNode(node, false))}
          </div>
        </div>
      )}

      <div className="flex gap-2">
        {hasConfigChanged ? (
          <button
            type="button"
            onClick={() => void handleSaveAndRegenerate()}
            disabled={isDisabled}
            className="px-4 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-2"
          >
            {isRegenerating ? (
              <>
                <Spinner color="border-white" />
                Regenerating plan...
              </>
            ) : (
              "Save & Regenerate Plan"
            )}
          </button>
        ) : hasSkipChanged ? (
          <button
            type="button"
            onClick={() => void handleSkipOnlySave()}
            disabled={isDisabled}
            className="px-4 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {isSaving ? "Saving..." : "Save"}
          </button>
        ) : null}
        {(hasConfigChanged || hasSkipChanged) && (
          <button
            type="button"
            onClick={() => {
              setSelectedConfigPath(configPath ?? "");
              setSelectedSkippedSteps(new Set(skippedSteps));
            }}
            disabled={isDisabled}
            className="px-4 py-2 bg-gray-700 text-gray-300 rounded text-sm hover:bg-gray-600 disabled:opacity-50 disabled:cursor-not-allowed"
          >
            Reset
          </button>
        )}
      </div>
    </div>
  );
}
