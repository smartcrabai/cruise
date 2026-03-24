import { useCallback, useEffect, useId, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type {
  ChoiceDto,
  ConfigEntry,
  PlanEvent,
  Session,
  SessionPhase,
  WorkflowEvent,
  WorkspaceMode,
} from "./types";
import {
  approveSession,
  cancelSession,
  createSession,
  deleteSession,
  discardSession,
  fixSession,
  getSession,
  getSessionLog,
  getSessionPlan,
  listConfigs,
  listSessions,
  resetSession,
  respondToOption,
  runAllSessions,
  runSession,
} from "./lib/commands";
import { notifyDesktop } from "./lib/desktopNotifications";
import { DirectoryPicker } from "./components/DirectoryPicker";
import { MarkdownViewer } from "./components/MarkdownViewer";
import { PhaseBadge } from "./components/PhaseBadge";
import { SessionSidebar } from "./components/SessionSidebar";
import { getSessionActions } from "./lib/sessionActions";
import { formatLocalTime } from "./lib/format";

// ─── OptionDialog ─────────────────────────────────────────────────────────────

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
                onClick={() => onRespond({ nextStep: choice.next ?? undefined })}
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
                    placeholder="Type here…"
                    onKeyDown={(e) => {
                      if (e.key === "Enter")
                        onRespond({
                          nextStep: choice.next ?? undefined,
                          textInput: textValues[choice.label] ?? "",
                        });
                    }}
                  />
                  <button
                    type="button"
                    onClick={() =>
                      onRespond({
                        nextStep: choice.next ?? undefined,
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

// ─── WorkflowToastStack ───────────────────────────────────────────────────────

type ToastKind = "input-required" | "completed" | "failed";

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
};

const TOAST_LABEL: Record<ToastKind, string> = {
  "input-required": "Action required",
  "completed": "Completed",
  "failed": "Failed",
};

const TOAST_DURATION_MS: Record<ToastKind, number> = {
  "input-required": 10_000,
  "completed": 5_000,
  "failed": 5_000,
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
            aria-label="Dismiss"
            className="opacity-60 hover:opacity-100 flex-shrink-0 text-xs mt-0.5"
          >
            x
          </button>
        </div>
      ))}
    </div>
  );
}

// ─── ConfirmDialog ────────────────────────────────────────────────────────────

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

// ─── WorkflowRunner ───────────────────────────────────────────────────────────

interface WorkflowRunnerProps {
  session: Session;
  onSessionUpdated: (session: Session) => void;
  onSessionDeleted: () => void;
  onToast: (toast: Omit<WorkflowToast, "id">) => void;
}

interface PendingOption {
  requestId: string;
  choices: ChoiceDto[];
  plan?: string;
}

type ExecStatus = "idle" | "running" | "completed" | "failed" | "cancelled";
type ActiveTab = "info" | "plan" | "log";

function WorkflowRunner({ session, onSessionUpdated, onSessionDeleted, onToast }: WorkflowRunnerProps) {
  const uid = useId();
  const tabInfoId = `${uid}-tab-info`;
  const tabPlanId = `${uid}-tab-plan`;
  const tabLogId = `${uid}-tab-log`;
  const panelInfoId = `${uid}-panel-info`;
  const panelPlanId = `${uid}-panel-plan`;
  const panelLogId = `${uid}-panel-log`;

  const [status, setStatus] = useState<ExecStatus>("idle");
  const [currentStep, setCurrentStep] = useState<string | null>(null);
  const [liveLog, setLiveLog] = useState<string[]>([]);
  const [savedLog, setSavedLog] = useState<string>("");
  const [logLoading, setLogLoading] = useState(false);
  const [planContent, setPlanContent] = useState<string>("");
  const [planLoading, setPlanLoading] = useState(false);
  const [activeTab, setActiveTab] = useState<ActiveTab>("info");
  const [pendingOption, setPendingOption] = useState<PendingOption | null>(null);
  const [replanFeedback, setReplanFeedback] = useState("");
  const [replanPhase, setReplanPhase] = useState<"idle" | "editing" | "generating">("idle");
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

  // Scroll live log to bottom when new entries arrive
  useEffect(() => {
    if (status === "running") {
      logEndRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [liveLog, status]);

  function notifyEvent(kind: ToastKind, sessionInput: string, detail?: string) {
    onToast({ kind, sessionInput, detail: detail?.slice(0, 80) });
    void notifyDesktop("Cruise", `${TOAST_LABEL[kind]} — ${(detail ?? sessionInput).slice(0, 60)}`);
  }

  async function refreshSession() {
    const updated = await getSession(session.id);
    onSessionUpdated(updated);
  }

  async function handleApproveSession() {
    try {
      await approveSession(session.id);
      await refreshSession();
    } catch (e) {
      notifyEvent("failed", session.input, `Approve error: ${e}`);
    }
  }

  async function startRun(workspaceMode: WorkspaceMode) {
    setStatus("running");
    setCurrentStep(null);
    setLiveLog([]);
    setActiveTab("log");

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
        notifyEvent("input-required", session.input);
      } else if (event.event === "workflowCompleted") {
        setStatus("completed");
        setLiveLog((prev) => [
          ...prev,
          `✓ Completed — run: ${event.data.run}, skipped: ${event.data.skipped}, failed: ${event.data.failed}`,
        ]);
        notifyEvent("completed", session.input);
      } else if (event.event === "workflowFailed") {
        setStatus("failed");
        setLiveLog((prev) => [...prev, `✗ Failed: ${event.data.error}`]);
        notifyEvent("failed", session.input, event.data.error);
      } else if (event.event === "workflowCancelled") {
        setStatus("cancelled");
        setLiveLog((prev) => [...prev, "⏸ Cancelled"]);
      }
    };

    try {
      await runSession(session.id, workspaceMode, channel);
    } catch (e) {
      setStatus("failed");
      setLiveLog((prev) => [...prev, `Error: ${e}`]);
    }

    // Re-fetch session state and saved log in parallel after run resolves
    refreshSession().catch((e) => {
      setLiveLog((prev) => [...prev, `Session refresh error: ${e}`]);
    });

    void loadSavedLog();
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
    setPendingOption(null);
    try {
      await respondToOption(result);
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

    const channel = new Channel<PlanEvent>();
    channel.onmessage = (event) => {
      if (event.event === "planGenerated") {
        setPlanContent(event.data.content);
        setReplanPhase("idle");
        setReplanFeedback("");
        setActiveTab("plan");
      } else if (event.event === "planFailed") {
        setLiveLog((prev) => [...prev, `Replan failed: ${event.data.error}`]);
        setReplanPhase("editing");
      }
    };

    try {
      await fixSession({ sessionId: session.id, feedback: trimmed }, channel);
    } catch (e) {
      setLiveLog((prev) => [...prev, `Replan error: ${e}`]);
      setReplanPhase("editing");
    }
  }

  // Load saved log when switching to log tab (and not running)
  function handleTabChange(tab: ActiveTab) {
    setActiveTab(tab);
    if (tab === "log" && status !== "running") {
      void loadSavedLog();
    }
    if (tab === "plan" && !planContent) {
      void loadPlan();
    }
  }

  const actions = getSessionActions(session, status === "running" ? "running" : "idle");

  // Decide which log content to show
  const showLive = status === "running" || (status !== "idle" && liveLog.length > 0);
  const logContent = showLive ? liveLog.join("\n") : savedLog;

  return (
    <div className="h-full flex flex-col">
      {/* Header */}
      <div className="px-6 pt-6 pb-4 border-b border-gray-800 space-y-3">
        <div className="flex items-center gap-3">
          <h2 className="text-lg font-semibold font-mono text-gray-100">{session.id}</h2>
          <PhaseBadge phase={session.phase} planAvailable={session.planAvailable} />
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

        {/* Replan feedback */}
        {replanPhase === "editing" && (
          <div className="space-y-2">
            <textarea
              aria-label="Replan instructions"
              value={replanFeedback}
              onChange={(e) => setReplanFeedback(e.target.value)}
              rows={3}
              autoFocus
              placeholder="Describe the changes needed…"
              className="w-full bg-gray-900 border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 placeholder-gray-600 focus:border-blue-500 outline-none resize-none"
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void handleReplan();
              }}
            />
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
                onClick={() => { setReplanPhase("idle"); setReplanFeedback(""); }}
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
            Regenerating plan…
          </div>
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
          onClick={() => handleTabChange("info")}
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
          onClick={() => handleTabChange("plan")}
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
          onClick={() => handleTabChange("log")}
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
            {planLoading ? (
              <p className="p-4 text-xs text-gray-500">Loading plan…</p>
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
              <p className="p-4 text-xs text-gray-500">Loading log…</p>
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
          confirmLabel={deleting ? "Deleting…" : "Delete"}
          disabled={deleting}
          onConfirm={() => void handleDelete()}
          onCancel={() => setShowDeleteConfirm(false)}
        />
      )}
    </div>
  );
}

// ─── EmptyState ───────────────────────────────────────────────────────────────

function EmptyState() {
  return (
    <div className="h-full flex items-center justify-center">
      <p className="text-gray-600 text-sm">Select a session from the sidebar</p>
    </div>
  );
}

// ─── NewSessionForm ────────────────────────────────────────────────────────────

type PlanPhase = "input" | "generating" | "generated" | "fixing";

interface NewSessionFormProps {
  onCreated: (sessionId: string) => void;
}

function NewSessionForm({ onCreated }: NewSessionFormProps) {
  const [configs, setConfigs] = useState<ConfigEntry[]>([]);
  const [configPath, setConfigPath] = useState<string>("");
  const [baseDir, setBaseDir] = useState<string>("");
  const [input, setInput] = useState<string>("");
  const [planPhase, setPlanPhase] = useState<PlanPhase>("input");
  const [planContent, setPlanContent] = useState<string>("");
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<string>("");

  // Load configs and default base_dir on mount
  useEffect(() => {
    void listConfigs().then(setConfigs).catch(() => {});
    // Use the most recently updated session's baseDir as default
    void listSessions()
      .then((sessions) => {
        if (sessions.length > 0) {
          const latest = [...sessions].sort((a, b) =>
            (b.updatedAt ?? b.createdAt).localeCompare(a.updatedAt ?? a.createdAt)
          )[0];
          setBaseDir(latest.baseDir);
        }
      })
      .catch(() => {});
  }, []);

  async function handleGenerate() {
    if (!input.trim()) return;
    setError(null);
    setPlanPhase("generating");

    const channel = new Channel<PlanEvent>();
    channel.onmessage = (event) => {
      if (event.event === "planGenerated") {
        setPlanContent(event.data.content);
        setPlanPhase("generated");
      } else if (event.event === "planFailed") {
        setError(event.data.error);
        setPlanPhase("input");
      }
    };

    try {
      const id = await createSession(
        {
          input: input.trim(),
          configPath: configPath || undefined,
          baseDir: baseDir || ".",
        },
        channel
      );
      setSessionId(id);
    } catch (e) {
      setError(String(e));
      setPlanPhase("input");
    }
  }

  async function handleApprove() {
    if (!sessionId) return;
    setError(null);
    try {
      await approveSession(sessionId);
      onCreated(sessionId);
    } catch (e) {
      setError(String(e));
    }
  }

  async function handleDiscard() {
    if (!sessionId) return;
    setError(null);
    try {
      await discardSession(sessionId);
    } catch {
      // ignore discard errors
    }
    setSessionId(null);
    setPlanContent("");
    setPlanPhase("input");
    setFeedback("");
  }

  async function handleFix() {
    if (!sessionId || !feedback.trim()) return;
    setError(null);
    setPlanPhase("generating");

    const channel = new Channel<PlanEvent>();
    channel.onmessage = (event) => {
      if (event.event === "planGenerated") {
        setPlanContent(event.data.content);
        setPlanPhase("generated");
        setFeedback("");
      } else if (event.event === "planFailed") {
        setError(event.data.error);
        setPlanPhase("generated");
      }
    };

    try {
      await fixSession({ sessionId, feedback: feedback.trim() }, channel);
    } catch (e) {
      setError(String(e));
      setPlanPhase("generated");
    }
  }

  return (
    <div className="h-full flex flex-col">
      <div className="px-6 pt-6 pb-4 border-b border-gray-800">
        <h2 className="text-lg font-semibold text-gray-100">New Session</h2>
      </div>

      <div className={`flex-1 overflow-hidden p-6 ${planPhase === "generated" || planPhase === "fixing" ? "flex flex-col gap-4" : "overflow-auto space-y-5"}`}>
        {/* Error banner */}
        {error && (
          <div className="bg-red-900/40 border border-red-700 rounded px-4 py-3 text-sm text-red-300">
            {error}
          </div>
        )}

        {/* Input form */}
        {(planPhase === "input" || planPhase === "generating") && (
          <>
            {/* Config selector */}
            <div className="space-y-1.5">
              <label htmlFor="config-select" className="text-xs text-gray-500 uppercase tracking-wide">Config</label>
              <select
                id="config-select"
                value={configPath}
                onChange={(e) => setConfigPath(e.target.value)}
                disabled={planPhase === "generating"}
                className="w-full bg-gray-900 border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 focus:border-blue-500 outline-none disabled:opacity-50"
              >
                <option value="">Default (builtin)</option>
                {configs.map((c) => (
                  <option key={c.path} value={c.path}>
                    {c.name}
                  </option>
                ))}
              </select>
            </div>

            {/* Base dir */}
            <div className="space-y-1.5">
              <label htmlFor="base-dir-input" className="text-xs text-gray-500 uppercase tracking-wide">Working Directory</label>
              <DirectoryPicker
                id="base-dir-input"
                value={baseDir}
                onChange={setBaseDir}
                disabled={planPhase === "generating"}
                placeholder="e.g. /Users/you/projects/myapp"
              />
            </div>

            {/* Task input */}
            <div className="space-y-1.5">
              <label htmlFor="task-input" className="text-xs text-gray-500 uppercase tracking-wide">Task</label>
              <textarea
                id="task-input"
                value={input}
                onChange={(e) => setInput(e.target.value)}
                disabled={planPhase === "generating"}
                rows={4}
                placeholder="Describe what you want to implement…"
                className="w-full bg-gray-900 border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 placeholder-gray-600 focus:border-blue-500 outline-none resize-none disabled:opacity-50"
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void handleGenerate();
                }}
              />
            </div>

            <button
              type="button"
              onClick={() => void handleGenerate()}
              disabled={planPhase === "generating" || !input.trim()}
              className="px-5 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-2"
            >
              {planPhase === "generating" ? (
                <>
                  <span className="inline-block w-3 h-3 rounded-full border-2 border-white border-t-transparent animate-spin" />
                  Generating plan…
                </>
              ) : (
                "Generate plan"
              )}
            </button>
          </>
        )}

        {/* Plan review */}
        {(planPhase === "generated" || planPhase === "fixing") && (
          <>
            <div className="flex-1 flex flex-col min-h-0 gap-1.5">
              <span className="text-xs text-gray-500 uppercase tracking-wide">Generated Plan</span>
              <div className="flex-1 bg-gray-900 border border-gray-700 rounded overflow-auto min-h-0">
                <MarkdownViewer content={planContent} className="p-4" />
              </div>
            </div>

            {/* Fix feedback */}
            {planPhase === "fixing" && (
              <div className="space-y-1.5">
                <label htmlFor="fix-instructions-input" className="text-xs text-gray-500 uppercase tracking-wide">Fix Instructions</label>
                <textarea
                  id="fix-instructions-input"
                  value={feedback}
                  onChange={(e) => setFeedback(e.target.value)}
                  rows={3}
                  autoFocus
                  placeholder="Describe how to revise the plan…"
                  className="w-full bg-gray-900 border border-gray-700 rounded px-3 py-2 text-sm text-gray-200 placeholder-gray-600 focus:border-blue-500 outline-none resize-none"
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void handleFix();
                  }}
                />
                <div className="flex gap-2">
                  <button
                    type="button"
                    onClick={() => void handleFix()}
                    disabled={!feedback.trim()}
                    className="px-4 py-1.5 bg-blue-600 text-white rounded text-sm hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
                  >
                    Apply Fix
                  </button>
                  <button
                    type="button"
                    onClick={() => setPlanPhase("generated")}
                    className="px-4 py-1.5 border border-gray-700 text-gray-400 rounded text-sm hover:bg-gray-800"
                  >
                    Cancel
                  </button>
                </div>
              </div>
            )}

            {/* Action buttons */}
            {planPhase === "generated" && (
              <div className="flex gap-2">
                <button
                  type="button"
                  onClick={() => void handleApprove()}
                  className="px-4 py-2 bg-green-700 text-white rounded text-sm hover:bg-green-600"
                >
                  Approve
                </button>
                <button
                  type="button"
                  onClick={() => setPlanPhase("fixing")}
                  className="px-4 py-2 border border-gray-700 text-gray-300 rounded text-sm hover:bg-gray-800"
                >
                  Fix
                </button>
                <button
                  type="button"
                  onClick={() => void handleDiscard()}
                  className="px-4 py-2 border border-gray-700 text-red-400 rounded text-sm hover:bg-gray-800"
                >
                  Discard
                </button>
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}

// ─── RunAllView ───────────────────────────────────────────────────────────────

type RunAllStatus = "running" | "completed" | "cancelled" | "error";

interface RunAllSessionResult {
  sessionId: string;
  input: string;
  phase: SessionPhase;
  error?: string;
}

interface RunAllViewProps {
  onCompleted: () => void;
}

function RunAllView({ onCompleted }: RunAllViewProps) {
  const [status, setStatus] = useState<RunAllStatus>("running");
  const [total, setTotal] = useState(0);
  const [currentSession, setCurrentSession] = useState<{ id: string; input: string } | null>(null);
  const [currentStep, setCurrentStep] = useState<string | null>(null);
  const [results, setResults] = useState<RunAllSessionResult[]>([]);
  const [runError, setRunError] = useState<string | null>(null);
  const [pendingOption, setPendingOption] = useState<PendingOption | null>(null);
  const startedRef = useRef(false);
  const mountedRef = useRef(true);
  const channelRef = useRef<Channel<WorkflowEvent> | null>(null);

  async function startRunAll() {
    const channel = new Channel<WorkflowEvent>();
    channelRef.current = channel;

    channel.onmessage = (event) => {
      if (!mountedRef.current) return;
      if (event.event === "runAllStarted") {
        setTotal(event.data.total);
      } else if (event.event === "runAllSessionStarted") {
        setCurrentSession({ id: event.data.sessionId, input: event.data.input });
        setCurrentStep(null);
      } else if (event.event === "runAllSessionFinished") {
        const { sessionId, input, phase, error } = event.data;
        setResults((prev) => [...prev, { sessionId, input, phase, error }]);
        setCurrentSession(null);
        setCurrentStep(null);
        setPendingOption(null);
      } else if (event.event === "runAllCompleted") {
        setStatus(event.data.cancelled > 0 ? "cancelled" : "completed");
      } else if (event.event === "stepStarted") {
        setCurrentStep(event.data.step);
      } else if (event.event === "optionRequired") {
        setPendingOption({ requestId: event.data.requestId, choices: event.data.choices, plan: event.data.plan });
      }
    };

    try {
      await runAllSessions(channel);
    } catch (e) {
      if (mountedRef.current) {
        setStatus("error");
        setRunError(String(e));
      }
    }
  }

  useEffect(() => {
    mountedRef.current = true;
    if (startedRef.current) return;
    startedRef.current = true;
    // eslint-disable-next-line react-hooks/set-state-in-effect
    void startRunAll();
    return () => {
      mountedRef.current = false;
      if (channelRef.current) {
        channelRef.current.onmessage = () => {};
        channelRef.current = null;
      }
    };
  }, []);

  async function handleCancel() {
    try {
      await cancelSession();
    } catch (e) {
      setRunError(String(e));
    }
  }

  async function handleOptionRespond(result: { nextStep?: string; textInput?: string }) {
    setPendingOption(null);
    try {
      await respondToOption(result);
    } catch (e) {
      setRunError(String(e));
    }
  }

  return (
    <div className="h-full flex flex-col p-6 max-w-2xl mx-auto">
      <div className="flex items-center justify-between mb-6">
        <h2 className="text-xl font-semibold text-gray-100">Run All</h2>
        {status === "running" ? (
          <button
            type="button"
            onClick={() => void handleCancel()}
            className="px-3 py-1.5 text-sm border border-gray-700 text-gray-400 hover:bg-gray-800 rounded"
          >
            Cancel
          </button>
        ) : (
          <button
            type="button"
            onClick={onCompleted}
            className="px-3 py-1.5 text-sm bg-blue-600 text-white hover:bg-blue-700 rounded"
          >
            Done
          </button>
        )}
      </div>

      {total > 0 && (
        <div className="mb-4">
          <div className="flex justify-between text-xs text-gray-400 mb-1">
            <span>{results.length} / {total} sessions</span>
            {status === "running" && currentSession && (
              <span className="text-green-400 animate-pulse">Running…</span>
            )}
            {status === "completed" && <span className="text-green-400">Completed</span>}
            {status === "cancelled" && <span className="text-orange-400">Cancelled</span>}
            {status === "error" && <span className="text-red-400">Error</span>}
          </div>
          <div className="h-1.5 bg-gray-800 rounded-full overflow-hidden">
            <div
              className="h-full bg-blue-600 rounded-full transition-all duration-300"
              style={{ width: `${(results.length / total) * 100}%` }}
            />
          </div>
        </div>
      )}

      {status === "running" && currentSession && (
        <div className="mb-4 p-3 bg-gray-900 border border-green-900/50 rounded">
          <div className="flex items-center gap-2 mb-1">
            <span className="w-2 h-2 rounded-full bg-green-400 animate-pulse" />
            <span className="text-xs text-gray-400 font-mono">{currentSession.id}</span>
          </div>
          <p className="text-sm text-gray-200 truncate">{currentSession.input}</p>
          {currentStep && (
            <p className="text-xs text-gray-500 mt-1">
              {currentStep}
            </p>
          )}
        </div>
      )}

      <div className="flex-1 overflow-y-auto space-y-1">
        {results.map((r) => (
          <div
            key={r.sessionId}
            className="flex items-start gap-2 px-3 py-2 rounded bg-gray-900/50"
          >
            <span className="mt-0.5 text-sm">
              {r.phase === "Completed" && "✓"}
              {r.phase === "Failed" && "✗"}
              {r.phase === "Suspended" && "⏸"}
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
            <span className="text-green-400">{results.filter((r) => r.phase === "Completed").length} completed</span>
            {results.filter((r) => r.phase === "Failed").length > 0 && <span className="text-red-400">{results.filter((r) => r.phase === "Failed").length} failed</span>}
            {results.filter((r) => r.phase === "Suspended").length > 0 && <span className="text-orange-400">{results.filter((r) => r.phase === "Suspended").length} cancelled</span>}
          </div>
          {runError && <p className="text-xs text-red-400">{runError}</p>}
        </div>
      )}

      {pendingOption && (
        <OptionDialog
          choices={pendingOption.choices}
          plan={pendingOption.plan}
          onRespond={(result) => void handleOptionRespond(result)}
        />
      )}
    </div>
  );
}

// ─── App ──────────────────────────────────────────────────────────────────────

export default function App() {
  const [selectedSession, setSelectedSession] = useState<Session | null>(null);
  const [view, setView] = useState<"session" | "new" | "runAll">("session");
  const sidebarRefreshRef = useRef<(() => void) | null>(null);
  const [toasts, setToasts] = useState<WorkflowToast[]>([]);
  const toastIdRef = useRef(0);
  const toastTimersRef = useRef<Map<number, ReturnType<typeof setTimeout>>>(new Map());

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

  useEffect(() => {
    const timers = toastTimersRef.current;
    return () => {
      timers.forEach((timer) => clearTimeout(timer));
    };
  }, []);

  return (
    <div className="h-screen flex bg-gray-950 text-gray-100 font-sans">
      <WorkflowToastStack toasts={toasts} onDismiss={dismissToast} />
      {/* Sidebar */}
      <aside className="w-72 flex-shrink-0 border-r border-gray-800 flex flex-col">
        <SessionSidebar
          selectedId={selectedSession?.id ?? null}
          onSelect={(s) => { setSelectedSession(s); setView("session"); }}
          onNewSession={() => { setSelectedSession(null); setView("new"); }}
          onRunAll={() => { setSelectedSession(null); setView("runAll"); }}
          onRefreshRef={sidebarRefreshRef}
          onSelectedSessionUpdated={(s) => setSelectedSession(s)}
        />
      </aside>
      {/* Main content */}
      <main className="flex-1 overflow-auto">
        {view === "runAll" ? (
          <RunAllView
            onCompleted={() => {
              sidebarRefreshRef.current?.();
              setView("session");
            }}
          />
        ) : view === "new" ? (
          <NewSessionForm
            onCreated={(id) => {
              sidebarRefreshRef.current?.();
              // Navigate to the created session after a brief refresh
              setTimeout(() => {
                void getSession(id)
                  .then((session) => {
                    setSelectedSession(session);
                    setView("session");
                  })
                  .catch(() => {
                    setView("session");
                  });
              }, 300);
            }}
          />
        ) : selectedSession ? (
          <WorkflowRunner
            key={selectedSession.id}
            session={selectedSession}
            onSessionUpdated={(updated) => {
              setSelectedSession(updated);
              sidebarRefreshRef.current?.();
            }}
            onSessionDeleted={() => {
              setSelectedSession(null);
              sidebarRefreshRef.current?.();
            }}
            onToast={addToast}
          />
        ) : (
          <EmptyState />
        )}
      </main>
    </div>
  );
}
