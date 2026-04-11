import { useCallback, useEffect, useId, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type {
  AppConfig,
  ChoiceDto,
  ConfigEntry,
  PlanEvent,
  Session,
  SessionPhase,
  SkippableStepDto,
  WorkflowEvent,
  WorkspaceMode,
} from "./types";
import {
  approveSession,
  askSession,
  cancelSession,
  createSession,
  deleteSession,
  fixSession,
  getAppConfig,
  getNewSessionConfigDefaults,
  getNewSessionHistorySummary,
  getSession,
  getSessionLog,
  getSessionPlan,
  listConfigs,
  resetSession,
  respondToOption,
  runAllSessions,
  runSession,
  updateAppConfig,
} from "./lib/commands";
import { notifyDesktop } from "./lib/desktopNotifications";
import { DirectoryPicker } from "./components/DirectoryPicker";
import { MarkdownViewer } from "./components/MarkdownViewer";
import { PhaseBadge } from "./components/PhaseBadge";
import { SessionSidebar } from "./components/SessionSidebar";
import { getSessionActions, isApprovalReady, type RunStatus } from "./lib/sessionActions";
import {
  formatLocalTime,
  workflowEventLogLine,
  runAllCompletedLogLine,
  runAllSessionStartedLogLine,
  runAllStartedLogLine,
  runAllStepLogLine,
  runAllWorkflowEventLogLine,
  PHASE_ICON,
} from "./lib/format";

// --- OptionDialog ----------------------------------------------------------------

interface OptionDialogProps {
  choices: ChoiceDto[];
  plan?: string;
  onRespond: (result: { nextStep?: string; textInput?: string }) => void;
}

function OptionDialog({ choices, plan, onRespond }: OptionDialogProps) {
  const [textValues, setTextValues] = useState<Record<string, string>>({});
  const titleId = useId();

  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50">
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="bg-gray-900 rounded-lg shadow-xl border border-gray-700 p-6 max-w-lg w-full space-y-4"
      >
        <h2 id={titleId} className="text-lg font-semibold text-gray-100">Choose an option</h2>
        {plan && (
          <div className="bg-gray-800 border border-gray-700 rounded overflow-auto max-h-48">
            <MarkdownViewer content={plan} className="p-3" />
          </div>
        )}
        <div className="space-y-2">
          {choices.map((choice) =>
            choice.kind === "selector" ? (
              <button
                key={choice.label}
                type="button"
                onClick={() => onRespond({ nextStep: choice.next })}
                className="w-full text-left px-4 py-2 border border-gray-700 rounded hover:bg-gray-800 text-sm text-gray-200 transition-colors"
              >
                {choice.label}
              </button>
            ) : (
              <div key={choice.label} className="space-y-1">
                <label htmlFor={`text-input-${choice.label}`} className="text-sm text-gray-400">{choice.label}</label>
                <div className="flex gap-2">
                  <input
                    id={`text-input-${choice.label}`}
                    type="text"
                    value={textValues[choice.label] ?? ""}
                    onChange={(e) =>
                      setTextValues((prev) => ({
                        ...prev,
                        [choice.label]: e.target.value,
                      }))
                    }
                    className="flex-1 border border-gray-700 bg-gray-800 rounded px-3 py-1.5 text-sm text-gray-200 placeholder-gray-600 outline-none focus:border-blue-500"
                    placeholder="Type here..."
                    onKeyDown={(e) => {
                      if (e.key === "Enter")
                        onRespond({
                          nextStep: choice.next,
                          textInput: textValues[choice.label] ?? "",
                        });
                    }}
                  />
                  <button
                    type="button"
                    onClick={() =>
                      onRespond({
                        nextStep: choice.next,
                        textInput: textValues[choice.label] ?? "",
                      })
                    }
                    className="px-3 py-1.5 bg-blue-600 text-white rounded text-sm hover:bg-blue-700"
                  >
                    Submit
                  </button>
                </div>
              </div>
            )
          )}
        </div>
      </div>
    </div>
  );
}

// --- WorkflowToastStack ------------------------------------------------------------

type ToastKind = "input-required" | "completed" | "failed" | "plan-ready";

export interface WorkflowToast {
  id: number;
  kind: ToastKind;
  sessionInput: string;
  detail?: string;
}

const TOAST_STYLE: Record<ToastKind, string> = {
  "input-required": "border-amber-700 bg-amber-900/80 text-amber-100",
  "completed": "border-green-700 bg-green-900/80 text-green-100",
  "failed": "border-red-700 bg-red-900/80 text-red-100",
  "plan-ready": "border-blue-700 bg-blue-900/80 text-blue-100",
};

const TOAST_LABEL: Record<ToastKind, string> = {
  "input-required": "Action required",
  "completed": "Completed",
  "failed": "Failed",
  "plan-ready": "Plan ready",
};

const TOAST_DURATION_MS: Record<ToastKind, number> = {
  "input-required": 10_000,
  "completed": 5_000,
  "failed": 5_000,
  "plan-ready": 8_000,
};

export function WorkflowToastStack({
  toasts,
  onDismiss,
}: {
  toasts: WorkflowToast[];
  onDismiss: (id: number) => void;
}) {
  if (toasts.length === 0) return null;
  return (
    <div className="fixed bottom-4 right-4 z-50 flex flex-col gap-2 max-w-sm w-full pointer-events-none">
      {toasts.map((t) => (
        <div
          key={t.id}
          className={`flex items-start gap-3 px-4 py-3 rounded-lg border shadow-xl text-sm pointer-events-auto ${TOAST_STYLE[t.kind]}`}
        >
          <div className="flex-1 min-w-0">
            <div className="font-medium">{TOAST_LABEL[t.kind]}</div>
            <div className="text-xs opacity-75 truncate mt-0.5">{t.sessionInput}</div>
            {t.detail !== undefined && (
              <div data-testid="toast-detail" className="text-xs opacity-60 truncate mt-0.5">{t.detail}</div>
            )}
          </div>
          <button
            type="button"
            aria-label="Dismiss"
            onClick={() => onDismiss(t.id)}
            className="opacity-60 hover:opacity-100 flex-shrink-0 text-xs mt-0.5"
          >
            x
          </button>
        </div>
      ))}
    </div>
  );
}

// --- ConfirmDialog ----------------------------------------------------------------

interface ConfirmDialogProps {
  title: string;
  message: string;
  confirmLabel: string;
  disabled?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

function ConfirmDialog({ title, message, confirmLabel, disabled, onConfirm, onCancel }: ConfirmDialogProps) {
  const titleId = useId();
  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50">
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="bg-gray-900 rounded-lg shadow-xl border border-gray-700 p-6 max-w-sm w-full space-y-4"
      >
        <h2 id={titleId} className="text-lg font-semibold text-gray-100">{title}</h2>
        <p className="text-sm text-gray-400">{message}</p>
        <div className="flex gap-2 justify-end">
          <button
            type="button"
            onClick={onCancel}
            disabled={disabled}
            className="px-4 py-2 border border-gray-700 text-gray-400 rounded text-sm hover:bg-gray-800 disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onConfirm}
            disabled={disabled}
            className="px-4 py-2 bg-red-600 text-white rounded text-sm hover:bg-red-700 disabled:opacity-50"
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}

// --- SettingsModal ----------------------------------------------------------------

interface SettingsModalProps {
  initialParallelism: number;
  onSave: (config: AppConfig) => Promise<void>;
  onClose: () => void;
}

function SettingsModal({ initialParallelism, onSave, onClose }: SettingsModalProps) {
  const [value, setValue] = useState<number>(initialParallelism);
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const titleId = useId();

  async function handleSave() {
    if (Number.isNaN(value) || value <= 0) {
      setError("Must be at least 1");
      return;
    }
    setSaving(true);
    setError("");
    try {
      await onSave({ runAllParallelism: value });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setSaving(false);
    }
  }

  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50">
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="bg-gray-900 rounded-lg shadow-xl border border-gray-700 p-6 max-w-sm w-full space-y-4"
      >
        <h2 id={titleId} className="text-lg font-semibold text-gray-100">Settings</h2>
        <div className="space-y-1.5">
          <label htmlFor="run-all-parallelism" className="text-sm text-gray-400">
            Run All Parallelism
          </label>
          <input
            id="run-all-parallelism"
            type="number"
            min={1}
            value={Number.isNaN(value) ? "" : value}
            onChange={(e) => setValue(e.target.valueAsNumber)}
            className="w-full bg-gray-800 border border-gray-700 rounded px-3 py-1.5 text-sm text-gray-200 outline-none focus:border-blue-500"
          />
          {error && <p className="text-sm text-red-400">{error}</p>}
        </div>
        <div className="flex gap-2 justify-end">
          <button
            type="button"
            onClick={onClose}
            disabled={saving}
            className="px-4 py-2 border border-gray-700 text-gray-400 rounded text-sm hover:bg-gray-800 disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={() => void handleSave()}
            disabled={saving}
            className="px-4 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700 disabled:opacity-50"
          >
            {saving ? "Saving..." : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
}

// --- AskEditor --------------------------------------------------------------------

interface AskEditorProps {
  question: string;
  onQuestionChange: (value: string) => void;
  phase: "idle" | "editing" | "submitting";
  error: string;
  onSubmit: () => void;
  onCancel: () => void;
  className?: string;
}

function AskEditor({ question, onQuestionChange, phase, error, onSubmit, onCancel, className = "space-y-2" }: AskEditorProps) {
  return (
    <div className={className}>
      <textarea
        value={question}
        onChange={(e) => onQuestionChange(e.target.value)}
        rows={3}
        autoFocus
        placeholder="Ask a question about the plan..."
        className="w-full bg-gray-900 border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 placeholder-gray-600 focus:border-blue-500 outline-none resize-none"
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void onSubmit();
        }}
      />
      {error && (
        <p className="text-sm text-red-400">{error}</p>
      )}
      <div className="flex gap-2">
        <button
          type="button"
          onClick={() => void onSubmit()}
          disabled={phase === "submitting" || !question.trim()}
          className="px-4 py-1.5 bg-blue-600 text-white rounded text-sm hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
        >
          {phase === "submitting" ? "Asking..." : "Submit"}
        </button>
        <button
          type="button"
          onClick={onCancel}
          className="px-4 py-1.5 border border-gray-700 text-gray-400 rounded text-sm hover:bg-gray-800"
        >
          Cancel
        </button>
      </div>
    </div>
  );
}

// --- WorkflowRunner ---------------------------------------------------------------

interface WorkflowRunnerProps {
  session: Session;
  activeTab: ActiveTab;
  onActiveTabChange: (tab: ActiveTab) => void;
  onSessionUpdated: (session: Session) => void;
  onSessionDeleted: () => void;
  onToast: (toast: Omit<WorkflowToast, "id">) => void;
  onFixingChange: (sessionId: string, fixing: boolean) => void;
}

interface PendingOption {
  requestId: string;
  choices: ChoiceDto[];
  plan?: string;
}

type ActiveTab = "info" | "plan" | "log";

function WorkflowRunner({ session, activeTab, onActiveTabChange, onSessionUpdated, onSessionDeleted, onToast, onFixingChange }: WorkflowRunnerProps) {
  const uid = useId();
  const tabInfoId = `${uid}-tab-info`;
  const tabPlanId = `${uid}-tab-plan`;
  const tabLogId = `${uid}-tab-log`;
  const panelInfoId = `${uid}-panel-info`;
  const panelPlanId = `${uid}-panel-plan`;
  const panelLogId = `${uid}-panel-log`;

  const [status, setStatus] = useState<RunStatus>("idle");
  const [currentStep, setCurrentStep] = useState<string | null>(null);
  const [liveLog, setLiveLog] = useState<string[]>([]);
  const [savedLog, setSavedLog] = useState<string>("");
  const [logLoading, setLogLoading] = useState(false);
  const [planContent, setPlanContent] = useState<string>("");
  const [planLoading, setPlanLoading] = useState(false);
  const [pendingOption, setPendingOption] = useState<PendingOption | null>(null);
  const [replanFeedback, setReplanFeedback] = useState("");
  const [replanPhase, setReplanPhase] = useState<"idle" | "editing" | "generating">("idle");
  const [replanError, setReplanError] = useState("");
  const [askQuestion, setAskQuestion] = useState("");
  const [askPhase, setAskPhase] = useState<"idle" | "editing" | "submitting">("idle");
  const [askResponse, setAskResponse] = useState("");
  const [askError, setAskError] = useState("");
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const logEndRef = useRef<HTMLSpanElement | null>(null);

  // Load saved log from file when tab is opened or after run finishes
  const loadSavedLog = useCallback(async () => {
    setLogLoading(true);
    try {
      const content = await getSessionLog(session.id);
      setSavedLog(content);
    } catch (e) {
      setSavedLog(`(failed to load log: ${e})`);
    } finally {
      setLogLoading(false);
    }
  }, [session.id]);

  // Load plan content from file when plan tab is opened
  const loadPlan = useCallback(async () => {
    setPlanLoading(true);
    try {
      const content = await getSessionPlan(session.id);
      setPlanContent(content);
    } catch (e) {
      setPlanContent(`(failed to load plan: ${e})`);
    } finally {
      setPlanLoading(false);
    }
  }, [session.id]);

  // Reset transient state when the selected session changes.
  // activeTab is intentionally NOT reset here -- it is owned by App and persists
  // per session across navigation. Lazy-load is triggered by the effect below.
  useEffect(() => {
    setStatus("idle");
    setCurrentStep(null);
    setLiveLog([]);
    setSavedLog("");
    setPlanContent("");
    setPendingOption(null);
    setLogLoading(false);
    setReplanFeedback("");
    setReplanPhase("idle");
    setReplanError("");
    setAskQuestion("");
    setAskPhase("idle");
    setAskResponse("");
    setAskError("");
    setShowDeleteConfirm(false);
    setDeleting(false);
  }, [session.id]);

  // Clear the ephemeral fixingSessionIds entry from App when this runner unmounts.
  // The sidebar's "Fixing" badge now persists correctly via session.fixInProgress
  // (DTO field), so this only keeps the parent's in-memory set tidy.
  useEffect(() => {
    return () => {
      onFixingChange(session.id, false);
    };
  }, [session.id, onFixingChange]);

  useEffect(() => {
    if (activeTab === "log" && status !== "running") {
      void loadSavedLog();
    }
  }, [activeTab, loadSavedLog, status]);

  useEffect(() => {
    if (activeTab === "plan" && !planContent && session.planAvailable) {
      void loadPlan();
    }
  }, [activeTab, loadPlan, planContent, session.planAvailable]);


  useEffect(() => {
    if (status === "running") {
      logEndRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [liveLog, status]);

  async function refreshSession() {
    const updated = await getSession(session.id);
    onSessionUpdated(updated);
  }

  async function handleApproveSession() {
    try {
      await approveSession(session.id);
      await refreshSession();
    } catch (e) {
      onToast({ kind: "failed", sessionInput: session.input, detail: `Approve error: ${e}` });
    }
  }

  async function startRun(workspaceMode: WorkspaceMode) {
    setStatus("running");
    setCurrentStep(null);
    setLiveLog([]);
    onActiveTabChange("log");

    const channel = new Channel<WorkflowEvent>();

    channel.onmessage = (event) => {
      if (event.event === "stepStarted") {
        setCurrentStep(event.data.step);
        setLiveLog((prev) => [...prev, event.data.step]);
      } else if (event.event === "optionRequired") {
        setPendingOption({
          requestId: event.data.requestId,
          choices: event.data.choices,
          plan: event.data.plan,
        });
        onToast({ kind: "input-required", sessionInput: session.input });
      } else if (event.event === "workflowCompleted") {
        setStatus("completed");
        setLiveLog((prev) => [...prev, workflowEventLogLine(event)]);
      } else if (event.event === "workflowFailed") {
        setStatus("failed");
        setLiveLog((prev) => [...prev, workflowEventLogLine(event)]);
      } else if (event.event === "workflowCancelled") {
        setStatus("cancelled");
        setLiveLog((prev) => [...prev, workflowEventLogLine(event)]);
      }
    };

    try {
      await runSession(session.id, workspaceMode, channel);
    } catch (e) {
      setStatus("failed");
      setLiveLog((prev) => [...prev, `Error: ${e}`]);
    }

    // Re-fetch session state after run resolves. The log tab effect reloads
    // persisted log content once the run leaves the "running" state.
    refreshSession().catch((e) => {
      setLiveLog((prev) => [...prev, `Session refresh error: ${e}`]);
    });
  }

  async function handleCancel() {
    try {
      await cancelSession();
    } catch (e) {
      setLiveLog((prev) => [...prev, `Cancel error: ${e}`]);
    }
  }

  async function handleReset() {
    try {
      const updated = await resetSession(session.id);
      onSessionUpdated(updated);
      setStatus("idle");
      setCurrentStep(null);
      setLiveLog([]);
    } catch (e) {
      setLiveLog((prev) => [...prev, `Reset error: ${e}`]);
    }
  }

  async function handleDelete() {
    setDeleting(true);
    try {
      await deleteSession(session.id);
      onSessionDeleted();
    } catch (e) {
      setLiveLog((prev) => [...prev, `Delete error: ${e}`]);
      setShowDeleteConfirm(false);
    } finally {
      setDeleting(false);
    }
  }

  async function handleOptionRespond(result: {
    nextStep?: string;
    textInput?: string;
  }) {
    const sessionId = pendingOption?.requestId;
    setPendingOption(null);
    if (!sessionId) return;
    try {
      await respondToOption(sessionId, result);
      // Re-sync after awaiting_input = false is saved
      await refreshSession();
    } catch (e) {
      setLiveLog((prev) => [...prev, `Option response error: ${e}`]);
    }
  }

  async function handleReplan() {
    const trimmed = replanFeedback.trim();
    if (!trimmed) return;
    setReplanPhase("generating");
    onFixingChange(session.id, true);
    setReplanError("");

    const channel = new Channel<PlanEvent>();
    channel.onmessage = (event) => {
      if (event.event === "planGenerating") {
        void refreshSession();
      } else if (event.event === "planGenerated") {
        setPlanContent(event.data.content);
        setReplanPhase("idle");
        onFixingChange(session.id, false);
        setReplanFeedback("");
        setAskResponse("");
        onActiveTabChange("plan");
        void refreshSession();
      } else if (event.event === "planFailed") {
        setReplanError(event.data.error);
        setReplanPhase("editing");
        onFixingChange(session.id, false);
      }
    };

    try {
      await fixSession({ sessionId: session.id, feedback: trimmed }, channel);
    } catch (e) {
      setReplanError(String(e));
      setReplanPhase("editing");
      onFixingChange(session.id, false);
    }
  }

  async function handleAsk() {
    const trimmed = askQuestion.trim();
    if (!trimmed) return;
    setAskPhase("submitting");
    setAskError("");

    try {
      const answer = await askSession(session.id, trimmed);
      setAskResponse(answer);
      setAskPhase("idle");
      onActiveTabChange("plan");
    } catch (e) {
      setAskError(String(e));
      setAskPhase("editing");
    }
  }

  // isFixing drives the PhaseBadge header; includes the DTO field so the badge
  // stays correct after navigating away and back (replanPhase resets to "idle").
  const isFixing = replanPhase === "generating" || !!session.fixInProgress;
  // isApprovalReady() already checks !session.fixInProgress, so only the local
  // ephemeral flag needs to be forwarded to suppress buttons while the request
  // is in flight within this component instance.
  const actions = getSessionActions(session, status, replanPhase === "generating");
  const notBusy = replanPhase === "idle" && askPhase === "idle";
  const canShowFix = actions.showFix && notBusy;
  const canShowAsk = actions.showAsk && notBusy;

  // Decide which log content to show
  const showLive = status === "running" || (status !== "idle" && liveLog.length > 0);
  const logContent = showLive ? liveLog.join("\n") : savedLog;

  return (
    <div className="h-full flex flex-col">
      {/* Header */}
      <div className="px-6 pt-6 pb-4 border-b border-gray-800 space-y-3">
        <div className="flex items-center gap-3">
          <h2 className="text-lg font-semibold font-mono text-gray-100">{session.id}</h2>
          <PhaseBadge phase={session.phase} planAvailable={session.planAvailable} fixing={isFixing} />
        </div>

        {session.prUrl && (
          <button
            type="button"
            onClick={() => void openUrl(session.prUrl!)}
            aria-label="Open Pull Request in browser"
            className="inline-flex items-center gap-1.5 text-sm text-blue-400 hover:text-blue-300 hover:underline"
          >
            PR: {session.prUrl.split("/").slice(-2).join(" #")}
            <span className="text-xs">↗</span>
          </button>
        )}

        <div className="text-sm text-gray-400 italic">{session.input}</div>

        {/* Controls */}
        <div className="flex gap-2">
          {actions.showApprove && (
            <button
              type="button"
              onClick={() => void handleApproveSession()}
              className="px-4 py-2 bg-green-700 text-white rounded text-sm hover:bg-green-600"
            >
              Approve
            </button>
          )}
          {canShowFix && (
            <button
              type="button"
              onClick={() => setReplanPhase("editing")}
              className="px-4 py-2 border border-gray-700 text-gray-300 rounded text-sm hover:bg-gray-800"
            >
              Fix
            </button>
          )}
          {canShowAsk && (
            <button
              type="button"
              onClick={() => setAskPhase("editing")}
              className="px-4 py-2 border border-gray-700 text-gray-300 rounded text-sm hover:bg-gray-800"
            >
              Ask
            </button>
          )}
          {actions.showCreateWorktree && (
            <>
              <button
                type="button"
                onClick={() => void startRun("Worktree")}
                className="px-4 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700"
              >
                Create worktree (new branch)
              </button>
              <button
                type="button"
                onClick={() => void startRun("CurrentBranch")}
                className="px-4 py-2 border border-gray-700 text-gray-200 rounded text-sm hover:bg-gray-800"
              >
                Use current branch
              </button>
            </>
          )}
          {actions.showRun && (
            <button
              type="button"
              onClick={() => void startRun(session.workspaceMode)}
              className="px-4 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700"
            >
              {actions.runLabel}
            </button>
          )}
          {actions.showCancel && (
            <button
              type="button"
              onClick={() => void handleCancel()}
              className="px-4 py-2 bg-red-600 text-white rounded text-sm hover:bg-red-700"
            >
              Cancel
            </button>
          )}
          {actions.showReset && (
            <button
              type="button"
              onClick={() => void handleReset()}
              className="px-4 py-2 border border-gray-700 text-orange-400 rounded text-sm hover:bg-gray-800"
            >
              Reset to Planned
            </button>
          )}
          {actions.showReplan && replanPhase !== "generating" && (
            <button
              type="button"
              onClick={() => setReplanPhase("editing")}
              className="px-4 py-2 border border-gray-700 text-gray-300 rounded text-sm hover:bg-gray-800"
            >
              Replan
            </button>
          )}
          {actions.showDelete && (
            <button
              type="button"
              onClick={() => setShowDeleteConfirm(true)}
              className="px-4 py-2 border border-gray-700 text-red-400 rounded text-sm hover:bg-red-900/30"
            >
              Delete
            </button>
          )}
        </div>

        {/* Replan / Fix feedback */}
        {replanPhase === "editing" && (
          <div className="space-y-2">
            <textarea
              aria-label="Replan instructions"
              value={replanFeedback}
              onChange={(e) => setReplanFeedback(e.target.value)}
              rows={3}
              autoFocus
              placeholder="Describe the changes needed..."
              className="w-full bg-gray-900 border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 placeholder-gray-600 focus:border-blue-500 outline-none resize-none"
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void handleReplan();
              }}
            />
            {replanError && (
              <p className="text-sm text-red-400">{replanError}</p>
            )}
            <div className="flex gap-2">
              <button
                type="button"
                onClick={() => void handleReplan()}
                disabled={!replanFeedback.trim()}
                className="px-4 py-1.5 bg-blue-600 text-white rounded text-sm hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
              >
                Apply
              </button>
              <button
                type="button"
                onClick={() => { setReplanPhase("idle"); setReplanFeedback(""); setReplanError(""); }}
                className="px-4 py-1.5 border border-gray-700 text-gray-400 rounded text-sm hover:bg-gray-800"
              >
                Cancel
              </button>
            </div>
          </div>
        )}
        {replanPhase === "generating" && (
          <div className="flex items-center gap-2 text-sm text-gray-400">
            <span className="inline-block w-3 h-3 rounded-full border-2 border-gray-400 border-t-transparent animate-spin" />
            Regenerating plan...
          </div>
        )}

        {/* Ask editor */}
        {askPhase !== "idle" && (
          <AskEditor
            question={askQuestion}
            onQuestionChange={setAskQuestion}
            phase={askPhase}
            error={askError}
            onSubmit={handleAsk}
            onCancel={() => { setAskPhase("idle"); setAskQuestion(""); setAskError(""); }}
          />
        )}

        {/* Progress indicator */}
        {status === "running" && currentStep && (
          <div className="text-sm text-gray-400">
            Step:{" "}
            <span className="font-medium text-gray-200">{currentStep}</span>
          </div>
        )}
      </div>

      {/* Tabs */}
      <div role="tablist" className="flex border-b border-gray-800">
        <button
          type="button"
          role="tab"
          id={tabInfoId}
          aria-selected={activeTab === "info"}
          aria-controls={panelInfoId}
          onClick={() => onActiveTabChange("info")}
          className={`px-4 py-2 text-xs font-medium transition-colors ${
            activeTab === "info"
              ? "text-blue-400 border-b-2 border-blue-500"
              : "text-gray-500 hover:text-gray-300"
          }`}
        >
          Info
        </button>
        <button
          type="button"
          role="tab"
          id={tabPlanId}
          aria-selected={activeTab === "plan"}
          aria-controls={panelPlanId}
          onClick={() => onActiveTabChange("plan")}
          className={`px-4 py-2 text-xs font-medium transition-colors ${
            activeTab === "plan"
              ? "text-blue-400 border-b-2 border-blue-500"
              : "text-gray-500 hover:text-gray-300"
          }`}
        >
          Plan
        </button>
        <button
          type="button"
          role="tab"
          id={tabLogId}
          aria-selected={activeTab === "log"}
          aria-controls={panelLogId}
          onClick={() => onActiveTabChange("log")}
          className={`px-4 py-2 text-xs font-medium transition-colors ${
            activeTab === "log"
              ? "text-blue-400 border-b-2 border-blue-500"
              : "text-gray-500 hover:text-gray-300"
          }`}
        >
          Log
          {status === "running" && (
            <span className="ml-1.5 inline-block w-1.5 h-1.5 rounded-full bg-green-400 animate-pulse" />
          )}
        </button>
      </div>

      {/* Tab content */}
      <div className="flex-1 overflow-auto">
        {activeTab === "info" && (
          <div id={panelInfoId} role="tabpanel" aria-labelledby={tabInfoId} className="p-6 space-y-3 text-sm text-gray-400">
            <div>
              <span className="text-gray-600 text-xs uppercase tracking-wide">Config</span>
              <p className="font-mono text-gray-300 mt-0.5">{session.configSource}</p>
            </div>
            <div>
              <span className="text-gray-600 text-xs uppercase tracking-wide">Base dir</span>
              <p className="font-mono text-gray-300 mt-0.5">{session.baseDir}</p>
            </div>
            {session.worktreeBranch && (
              <div>
                <span className="text-gray-600 text-xs uppercase tracking-wide">Branch</span>
                <p className="font-mono text-gray-300 mt-0.5">{session.worktreeBranch}</p>
              </div>
            )}
            <div>
              <span className="text-gray-600 text-xs uppercase tracking-wide">Created</span>
              <p className="text-gray-300 mt-0.5">{formatLocalTime(session.createdAt)}</p>
            </div>
            {session.completedAt && (
              <div>
                <span className="text-gray-600 text-xs uppercase tracking-wide">Completed</span>
                <p className="text-gray-300 mt-0.5">{formatLocalTime(session.completedAt)}</p>
              </div>
            )}
            {session.phaseError && (
              <div>
                <span className="text-gray-600 text-xs uppercase tracking-wide">Error</span>
                <p className="text-red-400 mt-0.5 font-mono text-xs">{session.phaseError}</p>
              </div>
            )}
          </div>
        )}

        {activeTab === "plan" && (
          <div id={panelPlanId} role="tabpanel" aria-labelledby={tabPlanId} className="h-full overflow-auto">
            {askResponse && (
              <div className="border-b border-gray-800 px-6 py-4 bg-gray-900/50">
                <div className="text-xs text-gray-500 uppercase tracking-wide mb-2">Answer</div>
                <MarkdownViewer content={askResponse} className="" />
              </div>
            )}
            {planLoading ? (
              <p className="p-4 text-xs text-gray-500">Loading plan...</p>
            ) : planContent ? (
              <MarkdownViewer content={planContent} className="p-6" />
            ) : (
              <p className="p-4 text-xs text-gray-600">No plan available.</p>
            )}
          </div>
        )}

        {activeTab === "log" && (
          <div id={panelLogId} role="tabpanel" aria-labelledby={tabLogId} className="h-full flex flex-col">
            {logLoading && status !== "running" ? (
              <p className="p-4 text-xs text-gray-500">Loading log...</p>
            ) : logContent ? (
              <pre
                className="flex-1 text-xs font-mono bg-gray-950 text-gray-300 p-4 overflow-auto whitespace-pre-wrap leading-relaxed"
              >
                {logContent}
                <span ref={logEndRef} />
              </pre>
            ) : (
              <p className="p-4 text-xs text-gray-600">
                {status === "idle" ? "Run the session to see logs here." : "No log entries yet."}
              </p>
            )}
          </div>
        )}
      </div>

      {/* Option dialog */}
      {pendingOption && (
        <OptionDialog
          choices={pendingOption.choices}
          plan={pendingOption.plan}
          onRespond={(result) => void handleOptionRespond(result)}
        />
      )}

      {/* Delete confirmation dialog */}
      {showDeleteConfirm && (
        <ConfirmDialog
          title="Delete Session"
          message={`Delete session "${session.id}" and its worktree? This cannot be undone.`}
          confirmLabel={deleting ? "Deleting..." : "Delete"}
          disabled={deleting}
          onConfirm={() => void handleDelete()}
          onCancel={() => setShowDeleteConfirm(false)}
        />
      )}
    </div>
  );
}

// --- EmptyState -------------------------------------------------------------------

function EmptyState() {
  return (
    <div className="h-full flex items-center justify-center">
      <p className="text-gray-600 text-sm">Select a session from the sidebar</p>
    </div>
  );
}

// --- NewSessionForm ---------------------------------------------------------------

interface NewSessionDraft {
  input: string;
  configPath: string;
  baseDir: string;
  isGenerating: boolean;
  error: string | null;
}

function createInitialNewSessionDraft(): NewSessionDraft {
  return {
    input: "",
    configPath: "",
    baseDir: "",
    isGenerating: false,
    error: null,
  };
}

function collectExpandedStepIds(nodes: SkippableStepDto[]): Set<string> {
  const ids = new Set<string>();

  function visit(node: SkippableStepDto) {
    for (const id of node.expandedStepIds) {
      ids.add(id);
    }
    for (const child of node.children) {
      visit(child);
    }
  }

  for (const node of nodes) {
    visit(node);
  }

  return ids;
}

interface NewSessionFormProps {
  draft: NewSessionDraft;
  onDraftChange: (updater: (prev: NewSessionDraft) => NewSessionDraft) => void;
  onRefreshSidebar: () => void;
}

function NewSessionForm({ draft, onDraftChange, onRefreshSidebar }: NewSessionFormProps) {
  const [configs, setConfigs] = useState<ConfigEntry[]>([]);
  const [configSteps, setConfigSteps] = useState<SkippableStepDto[]>([]);
  const [skippedSteps, setSkippedSteps] = useState<Set<string>>(new Set());
  const [recentWorkingDirs, setRecentWorkingDirs] = useState<string[]>([]);
  const isMountedRef = useRef(true);

  const { input, configPath, baseDir, isGenerating, error } = draft;
  const configLookupBaseDir = configPath ? "." : (baseDir || ".");

  function set<K extends keyof NewSessionDraft>(key: K, value: NewSessionDraft[K]) {
    onDraftChange((prev) => ({ ...prev, [key]: value }));
  }

  useEffect(() => {
    isMountedRef.current = true;
    return () => {
      isMountedRef.current = false;
    };
  }, []);

  const refreshHistorySummary = useCallback(async () => {
    try {
      const summary = await getNewSessionHistorySummary();
      if (isMountedRef.current) {
        setRecentWorkingDirs(summary.recentWorkingDirs);
      }
      return summary;
    } catch {
      if (isMountedRef.current) {
        setRecentWorkingDirs([]);
      }
      return null;
    }
  }, []);

  useEffect(() => {
    let active = true;
    void getNewSessionConfigDefaults({
      baseDir: configLookupBaseDir,
      configPath: configPath || undefined,
    })
      .then((defaults) => {
        if (active) {
          const validStepIds = collectExpandedStepIds(defaults.steps);
          setConfigSteps(defaults.steps);
          setSkippedSteps(
            new Set(defaults.defaultSkippedSteps.filter((id) => validStepIds.has(id))),
          );
        }
      })
      .catch(() => {
        if (active) {
          setConfigSteps([]);
          setSkippedSteps(new Set());
        }
      });
    return () => { active = false; };
  }, [configLookupBaseDir, configPath]);

  function isParentChecked(node: SkippableStepDto): boolean {
    return node.expandedStepIds.every((id) => skippedSteps.has(id));
  }

  function isParentIndeterminate(node: SkippableStepDto): boolean {
    const selected = node.expandedStepIds.filter((id) => skippedSteps.has(id));
    return selected.length > 0 && selected.length < node.expandedStepIds.length;
  }

  function toggleStepIds(ids: Iterable<string>, checked: boolean) {
    setSkippedSteps((prev) => {
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
  }

  function stepNodeLabel(node: SkippableStepDto, isChild: boolean): string {
    if (!isChild) {
      return node.id;
    }
    const slash = node.id.lastIndexOf("/");
    return slash === -1 ? node.id : node.id.slice(slash + 1);
  }

  function renderStepNode(node: SkippableStepDto, isChild: boolean): React.ReactElement {
    const label = stepNodeLabel(node, isChild);

    if (node.children.length === 0) {
      return (
        <label key={node.id} className={`flex items-center gap-2 cursor-pointer${isChild ? " pl-6" : ""}`}>
          <input
            type="checkbox"
            checked={skippedSteps.has(node.id)}
            onChange={(e) => toggleStepIds([node.id], e.target.checked)}
            disabled={isGenerating}
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
            disabled={isGenerating}
            className="accent-blue-500"
          />
          <span className="text-sm text-gray-300 font-medium">{label}</span>
        </label>
        <div className="space-y-1 ml-4">
          {node.children.map((child) => renderStepNode(child, true))}
        </div>
      </div>
    );
  }

  // Load configs and history-backed defaults on mount.
  useEffect(() => {
    let active = true;
    void listConfigs().then(setConfigs).catch(() => {});
    void refreshHistorySummary()
      .then((summary) => {
        if (!active || !summary) return;
        onDraftChange((prev) => {
          let changed = false;
          const next = { ...prev };
          if (prev.baseDir === "" && summary.lastWorkingDir) {
            next.baseDir = summary.lastWorkingDir;
            changed = true;
          }
          if (prev.configPath === "" && summary.lastRequestedConfigPath) {
            next.configPath = summary.lastRequestedConfigPath;
            changed = true;
          }
          return changed ? next : prev;
        });
      })
      .catch((e) => {
        console.error("Failed to refresh history summary:", e);
      });
    return () => {
      active = false;
    };
  }, [onDraftChange, refreshHistorySummary]);

  async function handleGenerate() {
    if (!input.trim()) return;
    onDraftChange((prev) => ({ ...prev, error: null, isGenerating: true }));
    let formReleased = false;

    const channel = new Channel<PlanEvent>();
    channel.onmessage = (event) => {
      if (event.event === "sessionCreated") {
        formReleased = true;
        onDraftChange((prev) => ({
          ...createInitialNewSessionDraft(),
          baseDir: prev.baseDir,
          configPath: prev.configPath,
        }));
        void refreshHistorySummary();
        onRefreshSidebar();
      } else if (event.event === "planGenerated" || event.event === "planFailed") {
        onRefreshSidebar();
      }
    };

    try {
      await createSession(
        {
          input: input.trim(),
          configPath: configPath || undefined,
          baseDir: baseDir || ".",
          skippedSteps: Array.from(skippedSteps),
        },
        channel,
      );
    } catch (e) {
      if (!formReleased) {
        onDraftChange((prev) => ({ ...prev, error: String(e), isGenerating: false }));
      }
    }
  }

  return (
    <div className="h-full flex flex-col">
      <div className="px-6 pt-6 pb-4 border-b border-gray-800">
        <h2 className="text-lg font-semibold text-gray-100">New Session</h2>
      </div>

      <div className="flex-1 overflow-auto p-6 space-y-5">
        {/* Error banner */}
        {error && (
          <div className="bg-red-900/40 border border-red-700 rounded px-4 py-3 text-sm text-red-300">
            {error}
          </div>
        )}

        {/* Config selector */}
        <div className="space-y-1.5">
          <label htmlFor="config-select" className="text-xs text-gray-500 uppercase tracking-wide">Config</label>
          <select
            id="config-select"
            value={configPath}
            onChange={(e) => set("configPath", e.target.value)}
            disabled={isGenerating}
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

        {/* Skip steps */}
        {configSteps.length > 0 && (
          <div className="space-y-1.5">
            <label className="text-xs text-gray-500 uppercase tracking-wide">Skip Steps</label>
            <div className="space-y-1 max-h-40 overflow-y-auto">
              {configSteps.map((node) => renderStepNode(node, false))}
            </div>
          </div>
        )}

        {/* Base dir */}
        <div className="space-y-1.5">
          <label htmlFor="base-dir-input" className="text-xs text-gray-500 uppercase tracking-wide">Working Directory</label>
          <DirectoryPicker
            id="base-dir-input"
            value={baseDir}
            onChange={(v) => set("baseDir", v)}
            disabled={isGenerating}
            placeholder="e.g. /Users/you/projects/myapp"
          />
          {recentWorkingDirs.length > 0 && (
            <div className="relative z-[60] flex flex-wrap gap-2 pt-1">
              {recentWorkingDirs.map((dir) => (
                <button
                  key={dir}
                  type="button"
                  onClick={() => set("baseDir", dir)}
                  disabled={isGenerating}
                  className="px-2.5 py-1 rounded-full border border-gray-700 bg-gray-900 text-xs text-gray-300 hover:bg-gray-800 disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  {dir}
                </button>
              ))}
            </div>
          )}
        </div>

        {/* Task input */}
        <div className="space-y-1.5">
          <label htmlFor="task-input" className="text-xs text-gray-500 uppercase tracking-wide">Task</label>
          <textarea
            id="task-input"
            value={input}
            onChange={(e) => set("input", e.target.value)}
            disabled={isGenerating}
            rows={4}
            placeholder="Describe what you want to implement..."
            className="w-full bg-gray-900 border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 placeholder-gray-600 focus:border-blue-500 outline-none resize-none disabled:opacity-50"
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void handleGenerate();
            }}
          />
        </div>

        <button
          type="button"
          onClick={() => void handleGenerate()}
          disabled={isGenerating || !input.trim()}
          className="px-5 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-2"
        >
          {isGenerating ? (
            <>
              <span className="inline-block w-3 h-3 rounded-full border-2 border-white border-t-transparent animate-spin" />
              Creating session...
            </>
          ) : (
            "Generate plan"
          )}
        </button>
      </div>
    </div>
  );
}

// --- RunAllView ------------------------------------------------------------------

type RunAllStatus = "running" | "completed" | "cancelled" | "error";

interface RunAllSessionResult {
  sessionId: string;
  input: string;
  phase: SessionPhase;
  error?: string;
}

interface RunAllRunningSession {
  input: string;
  currentStep: string | null;
}

interface RunAllUiState {
  status: RunAllStatus;
  total: number;
  parallelism: number | null;
  runningSessions: Record<string, RunAllRunningSession>;
  results: RunAllSessionResult[];
  runError: string | null;
  pendingOptions: PendingOption[];
  liveLog: string[];
}

interface RunAllViewProps {
  state: RunAllUiState;
  onCancel: () => void;
  onOptionRespond: (result: { nextStep?: string; textInput?: string }) => void;
  onDone: () => void;
}

function RunAllView({ state, onCancel, onOptionRespond, onDone }: RunAllViewProps) {
  const { status, total, parallelism, runningSessions, results, runError, pendingOptions, liveLog } = state;
  const runningEntries = Object.entries(runningSessions);
  const progressCount = results.length + runningEntries.length;
  const { completedCount, failedCount, suspendedCount } = results.reduce(
    (acc, r) => {
      if (r.phase === "Completed") acc.completedCount++;
      else if (r.phase === "Failed") acc.failedCount++;
      else if (r.phase === "Suspended") acc.suspendedCount++;
      return acc;
    },
    { completedCount: 0, failedCount: 0, suspendedCount: 0 },
  );
  const visiblePendingOption = pendingOptions[0];
  const logEndRef = useRef<HTMLSpanElement | null>(null);

  useEffect(() => {
    if (status === "running") {
      logEndRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [liveLog, status]);

  return (
    <div className="h-full flex flex-col p-6 max-w-2xl mx-auto">
      <div className="flex items-center justify-between mb-6">
        <h2 className="text-xl font-semibold text-gray-100">Run All</h2>
        {status === "running" ? (
          <button
            type="button"
            onClick={onCancel}
            className="px-3 py-1.5 text-sm border border-gray-700 text-gray-400 hover:bg-gray-800 rounded"
          >
            Cancel
          </button>
        ) : (
          <button
            type="button"
            onClick={onDone}
            className="px-3 py-1.5 text-sm bg-blue-600 text-white hover:bg-blue-700 rounded"
          >
            Done
          </button>
        )}
      </div>

      {(total > 0 || status === "error") && (
        <div className="mb-4">
          <div className="flex justify-between text-xs text-gray-400 mb-1">
            {total > 0 && <span>{progressCount} / {total} sessions</span>}
            {status === "running" && runningEntries.length > 0 && (
              <span className="text-green-400 animate-pulse">
                Running {runningEntries.length}
                {parallelism !== null && ` / ${parallelism}`}
              </span>
            )}
            {status === "completed" && <span className="text-green-400">Completed</span>}
            {status === "cancelled" && <span className="text-orange-400">Cancelled</span>}
            {status === "error" && <span className="text-red-400">Error</span>}
          </div>
          {total > 0 && (
            <div className="h-1.5 bg-gray-800 rounded-full overflow-hidden">
              <div
                className="h-full bg-blue-600 rounded-full transition-all duration-300"
                style={{ width: `${(progressCount / total) * 100}%` }}
              />
            </div>
          )}
        </div>
      )}

      {status === "running" && runningEntries.length > 0 && (
        <div className="mb-4 space-y-2">
          {runningEntries.map(([sessionId, sessionState]) => (
            <div
              key={sessionId}
              className="p-3 bg-gray-900 border border-green-900/50 rounded"
            >
              <div className="flex items-center gap-2 mb-1">
                <span className="w-2 h-2 rounded-full bg-green-400 animate-pulse" />
                <span className="text-xs text-gray-400 font-mono">{sessionId}</span>
              </div>
              <p className="text-sm text-gray-200 truncate">{sessionState.input}</p>
              {sessionState.currentStep && (
                <p className="text-xs text-gray-500 mt-1">
                  {sessionState.currentStep}
                </p>
              )}
            </div>
          ))}
        </div>
      )}

      <pre className="mb-4 text-xs font-mono bg-gray-950 text-gray-300 p-4 rounded overflow-auto whitespace-pre-wrap leading-relaxed max-h-80">
        {liveLog.length > 0 ? liveLog.join("\n") : "Waiting for events..."}
        <span ref={logEndRef} />
      </pre>

      <div className="flex-1 overflow-y-auto space-y-1">
        {results.map((r) => (
          <div
            key={r.sessionId}
            className="flex items-start gap-2 px-3 py-2 rounded bg-gray-900/50"
          >
            <span className="mt-0.5 text-sm">
              {r.phase === "Completed" && PHASE_ICON.Completed}
              {r.phase === "Failed" && PHASE_ICON.Failed}
              {r.phase === "Suspended" && PHASE_ICON.Suspended}
            </span>
            <div className="flex-1 min-w-0">
              <p className="text-sm text-gray-300 truncate">{r.input}</p>
              {r.error && <p className="text-xs text-red-400 mt-0.5 truncate">{r.error}</p>}
            </div>
            <PhaseBadge phase={r.phase} />
          </div>
        ))}
      </div>

      {(status === "completed" || status === "cancelled" || status === "error") && (
        <div className="mt-4 p-3 bg-gray-900 border border-gray-800 rounded text-sm text-gray-400 flex flex-col gap-1">
          <div className="flex gap-4">
            <span className="text-green-400">{completedCount} completed</span>
            {failedCount > 0 && <span className="text-red-400">{failedCount} failed</span>}
            {suspendedCount > 0 && <span className="text-orange-400">{suspendedCount} cancelled</span>}
          </div>
          {runError && <p className="text-xs text-red-400">{runError}</p>}
        </div>
      )}

      {visiblePendingOption && (
        <OptionDialog
          choices={visiblePendingOption.choices}
          plan={visiblePendingOption.plan}
          onRespond={onOptionRespond}
        />
      )}
    </div>
  );
}

function appendLog(log: string[], line: string): string[] {
  const next = [...log, line];
  return next.length > 2000 ? next.slice(-2000) : next;
}

// --- App -------------------------------------------------------------------------

export default function App() {
  const [selectedSession, setSelectedSession] = useState<Session | null>(null);
  const [view, setView] = useState<"session" | "new" | "runAll">("session");
  const sidebarRefreshRef = useRef<(() => void) | null>(null);
  const [toasts, setToasts] = useState<WorkflowToast[]>([]);
  const [newSessionDraft, setNewSessionDraft] = useState<NewSessionDraft>(createInitialNewSessionDraft);
  const [sessionTabMap, setSessionTabMap] = useState<Record<string, ActiveTab>>({});
  const [runAllState, setRunAllState] = useState<RunAllUiState | null>(null);
  const runAllChannelRef = useRef<Channel<WorkflowEvent> | null>(null);
  const toastIdRef = useRef(0);
  const toastTimersRef = useRef<Map<number, ReturnType<typeof setTimeout>>>(new Map());
  const [fixingSessionIds, setFixingSessionIds] = useState<ReadonlySet<string>>(new Set());
  const [showSettings, setShowSettings] = useState(false);
  const [settingsParallelism, setSettingsParallelism] = useState<number>(1);

  async function handleOpenSettings() {
    try {
      const config = await getAppConfig();
      setSettingsParallelism(config.runAllParallelism);
      setShowSettings(true);
    } catch (e) {
      emitNotification("failed", "Settings", String(e));
    }
  }

  async function handleSaveSettings(config: AppConfig) {
    await updateAppConfig(config);
    setShowSettings(false);
    setRunAllState((prev) => {
      if (!prev || prev.status !== "running") return prev;
      if (prev.parallelism === config.runAllParallelism) return prev;
      return { ...prev, parallelism: config.runAllParallelism };
    });
  }

  const handleFixingChange = useCallback((sessionId: string, fixing: boolean) => {
    setFixingSessionIds((prev) => {
      if (prev.has(sessionId) === fixing) return prev;
      const next = new Set(prev);
      if (fixing) {
        next.add(sessionId);
      } else {
        next.delete(sessionId);
      }
      return next;
    });
  }, []);

  const sessionSnapshotRef = useRef<Map<string, Session> | null>(null);

  const addToast = useCallback((toast: Omit<WorkflowToast, "id">) => {
    const id = ++toastIdRef.current;
    setToasts((prev) => [...prev, { ...toast, id }]);
    const timer = setTimeout(() => {
      toastTimersRef.current.delete(id);
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, TOAST_DURATION_MS[toast.kind]);
    toastTimersRef.current.set(id, timer);
  }, []);

  const dismissToast = useCallback((id: number) => {
    const timer = toastTimersRef.current.get(id);
    if (timer !== undefined) {
      clearTimeout(timer);
      toastTimersRef.current.delete(id);
    }
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const emitNotification = useCallback((kind: ToastKind, sessionInput: string, detail?: string) => {
    addToast({ kind, sessionInput, detail: detail?.slice(0, 80) });
    void notifyDesktop("Cruise", `${TOAST_LABEL[kind]} -- ${(detail ?? sessionInput).slice(0, 60)}`);
  }, [addToast]);

  const handleSessionsChanged = useCallback((sessions: Session[]) => {
    const newMap = new Map(sessions.map((s) => [s.id, s]));
    const prevMap = sessionSnapshotRef.current;
    sessionSnapshotRef.current = newMap;
    if (prevMap === null) return; // first snapshot: suppress startup notifications
    for (const session of newMap.values()) {
      const prev = prevMap.get(session.id);
      const approvalReady = isApprovalReady(session);
      const wasApprovalReady = prev !== undefined && isApprovalReady(prev);
      if (approvalReady && !wasApprovalReady) {
        emitNotification("plan-ready", session.input);
      }
      const isCompleted = session.phase === "Completed";
      const wasCompleted = prev !== undefined && prev.phase === "Completed";
      if (isCompleted && !wasCompleted) {
        emitNotification("completed", session.input);
      }
      const isFailed = session.phase === "Failed";
      const wasFailed = prev !== undefined && prev.phase === "Failed";
      if (isFailed && !wasFailed) {
        emitNotification("failed", session.input, session.phaseError);
      }
    }
  }, [emitNotification]);

  useEffect(() => {
    const timers = toastTimersRef.current;
    return () => {
      timers.forEach((timer) => clearTimeout(timer));
      if (runAllChannelRef.current) {
        runAllChannelRef.current.onmessage = () => {};
        runAllChannelRef.current = null;
      }
    };
  }, []);

  function patchRunAll(updates: Partial<RunAllUiState>) {
    setRunAllState((prev) => prev ? { ...prev, ...updates } : prev);
  }

  function handleRunAll() {
    setSelectedSession(null);
    setView("runAll");
    if (runAllState) return;

    const channel = new Channel<WorkflowEvent>();
    runAllChannelRef.current = channel;
    setRunAllState({
      status: "running",
      total: 0,
      parallelism: null,
      runningSessions: {},
      results: [],
      runError: null,
      pendingOptions: [],
      liveLog: [],
    });

    channel.onmessage = (event) => {
      if (event.event === "runAllStarted") {
        setRunAllState((prev) => {
          if (!prev) return prev;
          const { total, parallelism } = event.data;
          return {
            ...prev,
            total,
            parallelism,
            liveLog: appendLog(prev.liveLog, runAllStartedLogLine(total, parallelism)),
          };
        });
      } else if (event.event === "runAllSessionStarted") {
        setRunAllState((prev) => {
          if (!prev) return prev;
          return {
            ...prev,
            runningSessions: {
              ...prev.runningSessions,
              [event.data.sessionId]: {
                input: event.data.input,
                currentStep: null,
              },
            },
            liveLog: appendLog(prev.liveLog, runAllSessionStartedLogLine(event.data.sessionId, event.data.input)),
          };
        });
      } else if (event.event === "runAllSessionFinished") {
        setRunAllState((prev) => {
          if (!prev) return prev;
          const { sessionId, input, phase, error } = event.data;
          const { [sessionId]: _finishedSession, ...remainingSessions } = prev.runningSessions;
          return {
            ...prev,
            runningSessions: remainingSessions,
            results: [...prev.results, { sessionId, input, phase, error }],
            pendingOptions: prev.pendingOptions.filter((option) => option.requestId !== sessionId),
          };
        });
        sidebarRefreshRef.current?.();
      } else if (event.event === "runAllCompleted") {
        setRunAllState((prev) => {
          if (!prev) return prev;
          return {
            ...prev,
            status: event.data.cancelled > 0 ? "cancelled" : "completed",
            liveLog: appendLog(prev.liveLog, runAllCompletedLogLine(event.data.cancelled)),
          };
        });
      } else if (event.event === "stepStarted") {
        setRunAllState((prev) => {
          if (!prev) return prev;
          const runningSession = prev.runningSessions[event.data.sessionId];
          return {
            ...prev,
            runningSessions: runningSession ? {
              ...prev.runningSessions,
              [event.data.sessionId]: {
                ...runningSession,
                currentStep: event.data.step,
              },
            } : prev.runningSessions,
            liveLog: appendLog(prev.liveLog, runAllStepLogLine(event.data.sessionId, event.data.step)),
          };
        });
      } else if (event.event === "optionRequired") {
        setRunAllState((prev) => {
          if (!prev) return prev;
          return {
            ...prev,
            pendingOptions: [
              ...prev.pendingOptions,
              { requestId: event.data.requestId, choices: event.data.choices, plan: event.data.plan },
            ],
          };
        });
      } else if (
        event.event === "workflowCompleted" ||
        event.event === "workflowFailed" ||
        event.event === "workflowCancelled"
      ) {
        setRunAllState((prev) => {
          if (!prev) return prev;
          return {
            ...prev,
            liveLog: appendLog(prev.liveLog, runAllWorkflowEventLogLine(event)),
          };
        });
      }
    };

    void runAllSessions(channel).catch((e) => {
      patchRunAll({ status: "error", runError: String(e) });
    });
  }

  async function handleRunAllCancel() {
    try {
      await cancelSession();
    } catch (e) {
      patchRunAll({ runError: String(e) });
    }
  }

  async function handleRunAllOptionRespond(result: { nextStep?: string; textInput?: string }) {
    const sessionId = runAllState?.pendingOptions[0]?.requestId;
    setRunAllState((prev) => prev ? { ...prev, pendingOptions: prev.pendingOptions.filter((o) => o.requestId !== sessionId) } : prev);
    if (!sessionId) return;
    try {
      await respondToOption(sessionId, result);
    } catch (e) {
      patchRunAll({ runError: String(e) });
    }
  }

  function handleRunAllDone() {
    if (runAllChannelRef.current) {
      runAllChannelRef.current.onmessage = () => {};
      runAllChannelRef.current = null;
    }
    setRunAllState(null);
    sidebarRefreshRef.current?.();
    setView("session");
  }

  return (
    <div className="h-screen flex bg-gray-950 text-gray-100 font-sans">
      <WorkflowToastStack toasts={toasts} onDismiss={dismissToast} />
      {showSettings && (
        <SettingsModal
          initialParallelism={settingsParallelism}
          onSave={handleSaveSettings}
          onClose={() => setShowSettings(false)}
        />
      )}
      {/* Sidebar */}
      <aside className="w-72 flex-shrink-0 border-r border-gray-800 flex flex-col">
        <SessionSidebar
          selectedId={selectedSession?.id ?? null}
          onSelect={(s) => { setSelectedSession(s); setView("session"); }}
          onNewSession={() => { setSelectedSession(null); setView("new"); }}
          onRunAll={handleRunAll}
          runAllActive={!!runAllState}
          onRefreshRef={sidebarRefreshRef}
          onSelectedSessionUpdated={(s) => setSelectedSession(s)}
          fixingSessionIds={fixingSessionIds}
          onSessionsChanged={handleSessionsChanged}
          onSettings={() => void handleOpenSettings()}
        />
      </aside>
      {/* Main content */}
      <main className="flex-1 overflow-auto">
        {view === "runAll" && runAllState ? (
          <RunAllView
            state={runAllState}
            onCancel={handleRunAllCancel}
            onOptionRespond={handleRunAllOptionRespond}
            onDone={handleRunAllDone}
          />
        ) : view === "new" ? (
          <NewSessionForm
            draft={newSessionDraft}
            onDraftChange={setNewSessionDraft}
            onRefreshSidebar={() => sidebarRefreshRef.current?.()}
          />
        ) : selectedSession ? (
          <WorkflowRunner
            key={selectedSession.id}
            session={selectedSession}
            activeTab={sessionTabMap[selectedSession.id] ?? "plan"}
            onActiveTabChange={(tab) =>
              setSessionTabMap((prev) => ({ ...prev, [selectedSession.id]: tab }))
            }
            onSessionUpdated={(updated) => {
              setSelectedSession(updated);
              sidebarRefreshRef.current?.();
            }}
            onSessionDeleted={() => {
              setSessionTabMap((prev) => {
                const next = { ...prev };
                delete next[selectedSession.id];
                return next;
              });
              setSelectedSession(null);
              sidebarRefreshRef.current?.();
            }}
            onToast={(toast) => emitNotification(toast.kind, toast.sessionInput, toast.detail)}
            onFixingChange={handleFixingChange}
          />
        ) : (
          <EmptyState />
        )}
      </main>
    </div>
  );
}
