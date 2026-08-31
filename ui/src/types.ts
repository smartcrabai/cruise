// --- Session ------------------------------------------------------------------

export type SessionPhase =
  | "Draft"
  | "Awaiting Approval"
  | "Awaiting Input"
  | "Planned"
  | "Running"
  | "Completed"
  | "Failed"
  | "Suspended";

export type WorkspaceMode = "Worktree" | "CurrentBranch";

export interface Session {
  id: string;
  phase: SessionPhase;
  phaseError?: string;
  configSource: string;
  configPath?: string | null;
  baseDir: string;
  repo?: string;
  input: string;
  title?: string;
  currentStep?: string | null;
  createdAt: string;
  completedAt?: string;
  worktreeBranch?: string;
  workspaceMode: WorkspaceMode;
  prUrl?: string;
  updatedAt?: string | null;
  awaitingInput?: boolean;
  pendingAskQuestion?: string | null;
  planError?: string | null;
  fixInProgress?: boolean;
  exec?: boolean;
  planAvailable?: boolean;
  skippedSteps: string[];
}

// --- IPC events ---------------------------------------------------------------
export type OperationKind = "generate" | "fix" | "ask" | "replan" | "run" | "batchQueued" | "batchRun" | "mutate";
export type EventStream = "stdout" | "stderr" | "info";

export interface OptionChoicePayload {
  label: string;
  kind: "selector" | "textInput";
  nextStep?: string;
}
export type ChoiceDto = OptionChoicePayload;

export interface PendingPrompt {
  requestId: string;
  sessionId: string;
  kind: "ask" | "option";
  question: string | null;
  choices: OptionChoicePayload[];
}

export type ApplicationEvent =
  | { event: "planStarted"; data: { sessionId: string; operation: OperationKind } }
  | { event: "planChunk"; data: { sessionId: string; stream: EventStream; text: string } }
  | { event: "askUserRequired"; data: { sessionId: string; requestId: string; question: string } }
  | { event: "planFinished"; data: { sessionId: string; phase: string } }
  | { event: "planFailed"; data: { sessionId: string; error: string } }
  | { event: "planCancelled"; data: { sessionId: string } }
  | { event: "runStarted"; data: { sessionId: string } }
  | { event: "runPhase"; data: { sessionId: string; phase: string } }
  | { event: "stepStarted"; data: { sessionId: string; step: string } }
  | { event: "optionRequired"; data: { sessionId: string; requestId: string; prompt: string; choices: OptionChoicePayload[] } }
  | { event: "prCreated"; data: { sessionId: string; url: string } }
  | { event: "runFinished"; data: { sessionId: string; phase: string } }
  | { event: "runFailed"; data: { sessionId: string; error: string } }
  | { event: "runCancelled"; data: { sessionId: string } }
  | { event: "batchStarted"; data: { total: number; parallelism: number } }
  | { event: "batchTotalChanged"; data: { total: number } }
  | { event: "batchSessionStarted"; data: { id: string } }
  | { event: "batchSessionFinished"; data: { id: string; phase: string; error: string | null } }
  | { event: "batchFinished"; data: { cancelled: boolean } }
  | { event: "logChunk"; data: { sessionId: string | null; stream: EventStream; text: string; batch: boolean } };



// --- App config ---------------------------------------------------------------

export interface AppConfig {
  runAllParallelism: number;
}


// --- Cleanup ------------------------------------------------------------------

export interface CleanupResult {
  deleted: number;
  skipped: number;
  noPrDeleted?: number;
}

// --- Issue publishing -----------------------------------------------------------

export interface PublishedIssue {
  url: string;
  repo: string;
}

// --- Directory listing --------------------------------------------------------

export interface DirEntry {
  name: string;
  path: string;
}

// --- Session creation ---------------------------------------------------------

/**
 * Sentinel config value selecting the built-in default config.
 * Mirrors `crate::new_session_history::BUILTIN_CONFIG_KEY` in the Rust backend;
 * the resolver treats it as "use the built-in default", never as a file path.
 */
export const BUILTIN_CONFIG_PATH = "__builtin__";

/** Where a config file was discovered by the backend. */
export type ConfigEntrySource = "local" | "user";

export interface ConfigEntry {
  path: string;
  name: string;
  description?: string;
  /** Absent only for the synthetic "current config" entry injected by SessionConfigEditor. */
  source?: ConfigEntrySource;
}

export interface NewSessionHistorySummary {
  lastRequestedConfigPath?: string;
  lastWorkingDir?: string;
  recentWorkingDirs: string[];
}

export interface NewSessionConfigDefaults {
  steps: SkippableStepDto[];
  afterPrSteps: SkippableStepDto[];
  defaultSkippedSteps: string[];
  /** Present in current desktop responses; optional for older persisted fixtures. */
  resolvedConfigKey?: string;
}


// --- Update readiness ---------------------------------------------------------

export interface UpdateReadiness {
  canAutoUpdate: boolean;
  /** `"translocated"` | `"mountedVolume"` | `"unknownBundlePath"` - set when `canAutoUpdate` is false. */
  reason?: string;
  /** The resolved `.app` bundle path, for display in the UI. */
  bundlePath?: string;
  /** Human-readable remediation guidance. */
  guidance?: string;
}

// --- Skippable steps tree ------------------------------------------------------

export interface SkippableStepDto {
  id: string;
  expandedStepIds: string[];
  children: SkippableStepDto[];
}

// --- New Session Draft persistence --------------------------------------------

export interface NewSessionDraftPersisted {
  input: string;
  configPath?: string;
  baseDir: string;
  /** GitHub repository (owner/repo) selected instead of a directory. */
  repo?: string;
  skippedSteps: string[];
  updatedAt?: string;
}

export interface DagStepDto {
  name: string;
  kind: "prompt" | "command" | "option" | "unknown";
  isTerminal: boolean;
}

export interface DagEdgeDto {
  from: string;
  to: string | null;
  reason: string;
  selector: string | null;
}

export interface DagDto {
  startStep: string;
  steps: DagStepDto[];
  edges: DagEdgeDto[];
  currentStep?: string | null;
}
