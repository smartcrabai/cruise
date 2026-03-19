import { useCallback, useEffect, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Update } from "./lib/updater";
import { checkForUpdate, downloadAndInstall } from "./lib/updater";
import type { ChoiceDto, ConfigEntry, PlanEvent, Session, SessionPhase, WorkflowEvent } from "./types";
import {
  approveSession,
  cancelSession,
  cleanSessions,
  createSession,
  discardSession,
  fixSession,
  getSessionLog,
  getSessionPlan,
  listConfigs,
  listSessions,
  resetSession,
  respondToOption,
  runSession,
} from "./lib/commands";
import { DirectoryPicker } from "./components/DirectoryPicker";
import { MarkdownViewer } from "./components/MarkdownViewer";

// ─── Helpers ──────────────────────────────────────────────────────────────────

function formatLocalTime(iso: string): string {
  try {
    return new Date(iso).toLocaleString(undefined, {
      dateStyle: "short",
      timeStyle: "short",
    });
  } catch {
    return iso;
  }
}

// ─── Phase badge ──────────────────────────────────────────────────────────────

const PHASE_COLORS: Partial<Record<SessionPhase, string>> = {
  "Awaiting Approval": "bg-yellow-900/50 text-yellow-300",
  Planned: "bg-blue-900/50 text-blue-300",
  Running: "bg-green-900/50 text-green-300",
  Completed: "bg-gray-700/50 text-gray-300",
  Failed: "bg-red-900/50 text-red-300",
  Suspended: "bg-orange-900/50 text-orange-300",
};

function PhaseBadge({ phase }: { phase: SessionPhase }) {
  const cls = PHASE_COLORS[phase] ?? "bg-gray-700/50 text-gray-300";
  return (
    <span className={`px-2 py-0.5 rounded text-xs font-medium ${cls}`}>
      {phase}
    </span>
  );
}

// ─── SessionSidebar ───────────────────────────────────────────────────────────

interface SessionSidebarProps {
  selectedId: string | null;
  onSelect: (session: Session) => void;
  onNewSession: () => void;
  onRefreshRef?: React.MutableRefObject<(() => void) | null>;
}

function SessionSidebar({ selectedId, onSelect, onNewSession, onRefreshRef }: SessionSidebarProps) {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [cleaning, setCleaning] = useState(false);
  const [cleanMessage, setCleanMessage] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const sessions = await listSessions();
      const sorted = [...sessions].sort((a, b) => {
        const aInput = a.awaitingInput || a.phase === "Awaiting Approval";
        const bInput = b.awaitingInput || b.phase === "Awaiting Approval";
        if (aInput !== bInput) return aInput ? -1 : 1;
        const aTime = a.updatedAt ?? a.createdAt;
        const bTime = b.updatedAt ?? b.createdAt;
        return bTime.localeCompare(aTime);
      });
      setSessions(sorted);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (onRefreshRef) {
      onRefreshRef.current = load;
    }
  }, [load, onRefreshRef]);

  async function handleClean() {
    setCleaning(true);
    setCleanMessage(null);
    try {
      const result = await cleanSessions();
      setCleanMessage(`${result.deleted} deleted (skipped: ${result.skipped})`);
      void load();
    } catch (e) {
      setCleanMessage(`Error: ${e}`);
    } finally {
      setCleaning(false);
    }
  }

  return (
    <div className="h-full flex flex-col">
      {/* Sidebar header */}
      <div className="px-3 py-3 border-b border-gray-800 space-y-1.5">
        <div className="flex items-center justify-between gap-2">
          <h1 className="text-sm font-semibold text-gray-200">Sessions</h1>
          <div className="flex items-center gap-1">
            <button
              onClick={() => void load()}
              className="px-2 py-1 text-xs text-gray-400 hover:text-gray-200 hover:bg-gray-800 rounded"
              title="Refresh"
            >
              ↻
            </button>
            <button
              onClick={() => void handleClean()}
              disabled={cleaning}
              className="px-2 py-1 text-xs text-gray-400 hover:text-gray-200 hover:bg-gray-800 rounded disabled:opacity-50"
              title="Clean completed sessions"
            >
              {cleaning ? "…" : "Clean"}
            </button>
            <button
              onClick={onNewSession}
              className="px-2 py-1 text-xs bg-blue-600 text-white hover:bg-blue-700 rounded"
            >
              + New
            </button>
          </div>
        </div>
        {cleanMessage && (
          <p className="text-xs text-gray-400">{cleanMessage}</p>
        )}
      </div>

      {/* Session list */}
      <div className="flex-1 overflow-y-auto">
        {loading && (
          <p className="p-3 text-xs text-gray-500">Loading…</p>
        )}
        {error && (
          <p className="p-3 text-xs text-red-400">Error: {error}</p>
        )}
        {!loading && !error && sessions.length === 0 && (
          <p className="p-3 text-xs text-gray-500">No sessions found.</p>
        )}
        {sessions.map((s) => (
          <button
            key={s.id}
            onClick={() => onSelect(s)}
            className={`w-full text-left px-3 py-2.5 border-b border-gray-800/50 hover:bg-gray-800 transition-colors ${
              selectedId === s.id ? "bg-gray-800" : ""
            }`}
          >
            <div className="flex items-center justify-between gap-2 mb-0.5">
              <span className="text-xs text-gray-500 font-mono truncate">{s.id}</span>
              <PhaseBadge phase={s.phase} />
            </div>
            <p className="text-sm text-gray-300 truncate">{s.input}</p>
            <div className="flex items-center gap-1.5 mt-0.5">
              <span className="text-xs text-blue-400/70 font-mono truncate">
                {s.baseDir.replace(/\\/g, "/").split("/").filter(Boolean).at(-1) ?? s.baseDir}
              </span>
              <span className="text-xs text-gray-600">{formatLocalTime(s.updatedAt ?? s.createdAt)}</span>
            </div>
          </button>
        ))}
      </div>
    </div>
  );
}

// ─── OptionDialog ─────────────────────────────────────────────────────────────

interface OptionDialogProps {
  choices: ChoiceDto[];
  plan?: string;
  onRespond: (result: { nextStep?: string; textInput?: string }) => void;
}

function OptionDialog({ choices, plan, onRespond }: OptionDialogProps) {
  const [textValues, setTextValues] = useState<Record<string, string>>({});

  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50">
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="option-dialog-title"
        className="bg-gray-900 rounded-lg shadow-xl border border-gray-700 p-6 max-w-lg w-full space-y-4"
      >
        <h2 id="option-dialog-title" className="text-lg font-semibold text-gray-100">Choose an option</h2>
        {plan && (
          <div className="bg-gray-800 border border-gray-700 rounded overflow-auto max-h-48">
            <MarkdownViewer content={plan} className="p-3" />
          </div>
        )}
        <div className="space-y-2">
          {choices.map((choice, index) =>
            choice.kind === "selector" ? (
              <button
                key={index}
                onClick={() => onRespond({ nextStep: choice.next ?? undefined })}
                className="w-full text-left px-4 py-2 border border-gray-700 rounded hover:bg-gray-800 text-sm text-gray-200 transition-colors"
              >
                {choice.label}
              </button>
            ) : (
              <div key={index} className="space-y-1">
                <label className="text-sm text-gray-400">{choice.label}</label>
                <div className="flex gap-2">
                  <input
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

// ─── WorkflowRunner ───────────────────────────────────────────────────────────

interface WorkflowRunnerProps {
  session: Session;
  onSessionUpdated: (session: Session) => void;
}

interface StepEntry {
  name: string;
  index: number;
  total: number;
}

interface PendingOption {
  requestId: string;
  choices: ChoiceDto[];
  plan?: string;
}

type RunStatus = "idle" | "running" | "completed" | "failed" | "cancelled";
type ActiveTab = "info" | "plan" | "log";

function WorkflowRunner({ session, onSessionUpdated }: WorkflowRunnerProps) {
  const [status, setStatus] = useState<RunStatus>("idle");
  const [currentStep, setCurrentStep] = useState<StepEntry | null>(null);
  const [liveLog, setLiveLog] = useState<string[]>([]);
  const [savedLog, setSavedLog] = useState<string>("");
  const [logLoading, setLogLoading] = useState(false);
  const [planContent, setPlanContent] = useState<string>("");
  const [planLoading, setPlanLoading] = useState(false);
  const [activeTab, setActiveTab] = useState<ActiveTab>("info");
  const [pendingOption, setPendingOption] = useState<PendingOption | null>(null);
  const [replanFeedback, setReplanFeedback] = useState("");
  const [replanPhase, setReplanPhase] = useState<"idle" | "editing" | "generating">("idle");
  const _channelRef = useRef<Channel<WorkflowEvent> | null>(null);
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

  // Reset all state when the selected session changes
  useEffect(() => {
    setStatus("idle");
    setCurrentStep(null);
    setLiveLog([]);
    setSavedLog("");
    setPlanContent("");
    setPendingOption(null);
    setActiveTab("info");
    setLogLoading(false);
    setReplanFeedback("");
    setReplanPhase("idle");
    _channelRef.current = null;
  }, [session.id]);

  // Scroll live log to bottom when new entries arrive
  useEffect(() => {
    if (status === "running") {
      logEndRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [liveLog, status]);

  async function startRun() {
    setStatus("running");
    setCurrentStep(null);
    setLiveLog([]);
    setActiveTab("log");

    const channel = new Channel<WorkflowEvent>();
    _channelRef.current = channel;

    channel.onmessage = (event) => {
      if (event.event === "stepStarted") {
        setCurrentStep({
          name: event.data.step,
          index: event.data.index,
          total: event.data.total,
        });
        setLiveLog((prev) => [
          ...prev,
          `[${event.data.index + 1}/${event.data.total}] ${event.data.step}`,
        ]);
      } else if (event.event === "optionRequired") {
        setPendingOption({
          requestId: event.data.requestId,
          choices: event.data.choices,
          plan: event.data.plan,
        });
      } else if (event.event === "workflowCompleted") {
        setStatus("completed");
        setLiveLog((prev) => [
          ...prev,
          `✓ Completed — run: ${event.data.run}, skipped: ${event.data.skipped}, failed: ${event.data.failed}`,
        ]);
      } else if (event.event === "workflowFailed") {
        setStatus("failed");
        setLiveLog((prev) => [...prev, `✗ Failed: ${event.data.error}`]);
      } else if (event.event === "workflowCancelled") {
        setStatus("cancelled");
        setLiveLog((prev) => [...prev, "⏸ Cancelled"]);
      }
    };

    try {
      await runSession(session.id, channel);
    } catch (e) {
      setStatus("failed");
      setLiveLog((prev) => [...prev, `Error: ${e}`]);
    }

    // Reload saved log after run finishes
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

  async function handleOptionRespond(result: {
    nextStep?: string;
    textInput?: string;
  }) {
    setPendingOption(null);
    try {
      await respondToOption(result);
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

  const isRunnable =
    session.phase === "Planned" ||
    session.phase === "Running" ||
    session.phase === "Failed" ||
    session.phase === "Suspended";

  const isResettable =
    session.phase === "Running" ||
    session.phase === "Suspended" ||
    session.phase === "Failed" ||
    session.phase === "Completed";

  // Decide which log content to show
  const showLive = status === "running" || (status !== "idle" && liveLog.length > 0);
  const logContent = showLive ? liveLog.join("\n") : savedLog;

  return (
    <div className="h-full flex flex-col">
      {/* Header */}
      <div className="px-6 pt-6 pb-4 border-b border-gray-800 space-y-3">
        <div className="flex items-center gap-3">
          <h1 className="text-lg font-semibold font-mono text-gray-100">{session.id}</h1>
          <PhaseBadge phase={session.phase} />
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
          {isRunnable && status !== "running" && (
            <button
              onClick={() => void startRun()}
              className="px-4 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700"
            >
              {status === "idle" ? "Run" : "Re-run"}
            </button>
          )}
          {status === "running" && (
            <button
              onClick={() => void handleCancel()}
              className="px-4 py-2 bg-red-600 text-white rounded text-sm hover:bg-red-700"
            >
              Cancel
            </button>
          )}
          {isResettable && status !== "running" && (
            <button
              onClick={() => void handleReset()}
              className="px-4 py-2 border border-gray-700 text-orange-400 rounded text-sm hover:bg-gray-800"
            >
              Reset to Planned
            </button>
          )}
          {session.phase === "Planned" && status !== "running" && replanPhase !== "generating" && (
            <button
              onClick={() => setReplanPhase("editing")}
              className="px-4 py-2 border border-gray-700 text-gray-300 rounded text-sm hover:bg-gray-800"
            >
              Replan
            </button>
          )}
        </div>

        {/* Replan feedback */}
        {replanPhase === "editing" && (
          <div className="space-y-2">
            <textarea
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
                onClick={() => void handleReplan()}
                disabled={!replanFeedback.trim()}
                className="px-4 py-1.5 bg-blue-600 text-white rounded text-sm hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
              >
                Apply
              </button>
              <button
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
            Step {currentStep.index + 1}/{currentStep.total}:{" "}
            <span className="font-medium text-gray-200">{currentStep.name}</span>
          </div>
        )}
      </div>

      {/* Tabs */}
      <div className="flex border-b border-gray-800">
        <button
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
          <div className="p-6 space-y-3 text-sm text-gray-400">
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
          <div className="h-full overflow-auto">
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
          <div className="h-full flex flex-col">
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
        <h1 className="text-lg font-semibold text-gray-100">New Session</h1>
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
              <label className="text-xs text-gray-500 uppercase tracking-wide">Config</label>
              <select
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
              <label className="text-xs text-gray-500 uppercase tracking-wide">Working Directory</label>
              <DirectoryPicker
                value={baseDir}
                onChange={setBaseDir}
                disabled={planPhase === "generating"}
                placeholder="e.g. /Users/you/projects/myapp"
              />
            </div>

            {/* Task input */}
            <div className="space-y-1.5">
              <label className="text-xs text-gray-500 uppercase tracking-wide">Task</label>
              <textarea
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
                <label className="text-xs text-gray-500 uppercase tracking-wide">Fix Instructions</label>
                <textarea
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
                    onClick={() => void handleFix()}
                    disabled={!feedback.trim()}
                    className="px-4 py-1.5 bg-blue-600 text-white rounded text-sm hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
                  >
                    Apply Fix
                  </button>
                  <button
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
                  onClick={() => void handleApprove()}
                  className="px-4 py-2 bg-green-700 text-white rounded text-sm hover:bg-green-600"
                >
                  Approve
                </button>
                <button
                  onClick={() => setPlanPhase("fixing")}
                  className="px-4 py-2 border border-gray-700 text-gray-300 rounded text-sm hover:bg-gray-800"
                >
                  Fix
                </button>
                <button
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

// ─── UpdateNotification ──────────────────────────────────────────────────────

type UpdateState = "available" | "downloading" | "error";

function UpdateNotification() {
  const [update, setUpdate] = useState<Update | null>(null);
  const [state, setState] = useState<UpdateState>("available");
  const [progress, setProgress] = useState({ downloaded: 0, total: 0 });
  const [errorMsg, setErrorMsg] = useState("");

  useEffect(() => {
    const timer = setTimeout(() => {
      void checkForUpdate().then((u) => {
        if (u) setUpdate(u);
      });
    }, 2000);
    return () => clearTimeout(timer);
  }, []);

  if (!update) return null;

  async function handleInstall() {
    if (!update) return;
    setState("downloading");
    setProgress({ downloaded: 0, total: 0 });
    try {
      await downloadAndInstall(update, (chunk, contentLength) => {
        if (contentLength !== undefined) {
          setProgress({ downloaded: 0, total: contentLength });
        } else {
          setProgress((prev) => ({
            ...prev,
            downloaded: prev.downloaded + chunk,
          }));
        }
      });
    } catch (e) {
      setState("error");
      setErrorMsg(String(e));
    }
  }

  const pct = progress.total > 0 ? Math.round((progress.downloaded / progress.total) * 100) : 0;

  return (
    <div className="fixed top-4 right-4 z-50 w-80 bg-gray-900 border border-gray-700 rounded-lg shadow-xl p-4 space-y-3">
      {state === "available" && (
        <>
          <div className="text-sm text-gray-200 font-medium">
            Update available: v{update.version}
          </div>
          <div className="flex gap-2">
            <button
              onClick={() => void handleInstall()}
              className="px-3 py-1.5 bg-blue-600 text-white rounded text-sm hover:bg-blue-700"
            >
              Update Now
            </button>
            <button
              onClick={() => setUpdate(null)}
              className="px-3 py-1.5 border border-gray-700 text-gray-400 rounded text-sm hover:bg-gray-800"
            >
              Later
            </button>
          </div>
        </>
      )}
      {state === "downloading" && (
        <>
          <div className="text-sm text-gray-200">Downloading update…</div>
          <div className="w-full bg-gray-800 rounded-full h-2">
            <div
              className="bg-blue-500 h-2 rounded-full transition-all"
              style={{ width: `${pct}%` }}
            />
          </div>
          {progress.total > 0 && (
            <div className="text-xs text-gray-500">{pct}%</div>
          )}
        </>
      )}
      {state === "error" && (
        <>
          <div className="text-sm text-red-400">Update failed: {errorMsg}</div>
          <button
            onClick={() => setUpdate(null)}
            className="px-3 py-1.5 border border-gray-700 text-gray-400 rounded text-sm hover:bg-gray-800"
          >
            Dismiss
          </button>
        </>
      )}
    </div>
  );
}

// ─── App ──────────────────────────────────────────────────────────────────────

export default function App() {
  const [selectedSession, setSelectedSession] = useState<Session | null>(null);
  const [view, setView] = useState<"session" | "new">("session");
  const sidebarRefreshRef = useRef<(() => void) | null>(null);

  return (
    <div className="h-screen flex bg-gray-950 text-gray-100 font-sans">
      <UpdateNotification />
      {/* Sidebar */}
      <aside className="w-72 flex-shrink-0 border-r border-gray-800 flex flex-col">
        <SessionSidebar
          selectedId={selectedSession?.id ?? null}
          onSelect={(s) => { setSelectedSession(s); setView("session"); }}
          onNewSession={() => { setSelectedSession(null); setView("new"); }}
          onRefreshRef={sidebarRefreshRef}
        />
      </aside>
      {/* Main content */}
      <main className="flex-1 overflow-auto">
        {view === "new" ? (
          <NewSessionForm
            onCreated={(id) => {
              sidebarRefreshRef.current?.();
              // Navigate to the created session after a brief refresh
              setTimeout(async () => {
                try {
                  const { getSession } = await import("./lib/commands");
                  const session = await getSession(id);
                  setSelectedSession(session);
                  setView("session");
                } catch {
                  setView("session");
                }
              }, 300);
            }}
          />
        ) : selectedSession ? (
          <WorkflowRunner
            session={selectedSession}
            onSessionUpdated={(updated) => {
              setSelectedSession(updated);
              sidebarRefreshRef.current?.();
            }}
          />
        ) : (
          <EmptyState />
        )}
      </main>
    </div>
  );
}
