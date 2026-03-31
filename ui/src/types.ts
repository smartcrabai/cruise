// --- Session ------------------------------------------------------------------

export type SessionPhase =
  | "Awaiting Approval"
  | "Planned"
  | "Running"
  | "Completed"
  | "Failed"
  | "Suspended";

export type WorkspaceMode = "Worktree" | "CurrentBranch";

export interface Session {
  id: string;
  phase: SessionPhase;
  /** Populated when phase === "Failed" */
  phaseError?: string;
  configSource: string;
  baseDir: string;
  input: string;
  title?: string;
  currentStep?: string;
  createdAt: string;
  completedAt?: string;
  worktreeBranch?: string;
  workspaceMode: WorkspaceMode;
  prUrl?: string;
  updatedAt?: string;
  awaitingInput?: boolean;
  /** Whether a valid (non-empty) plan.md exists for this session. */
  planAvailable?: boolean;
}

// --- IPC Events ---------------------------------------------------------------

export interface StepStartedEvent {
  event: "stepStarted";
  data: { step: string };
}

export interface StepCompletedEvent {
  event: "stepCompleted";
  data: { step: string; success: boolean; durationMs: number; output?: string };
}

export interface ChoiceDto {
  label: string;
  kind: "selector" | "textInput";
  next?: string;
}

export interface OptionRequiredEvent {
  event: "optionRequired";
  data: { requestId: string; choices: ChoiceDto[]; plan?: string };
}

export interface WorkflowCompletedEvent {
  event: "workflowCompleted";
  data: { run: number; skipped: number; failed: number };
}

export interface WorkflowFailedEvent {
  event: "workflowFailed";
  data: { error: string };
}

export interface WorkflowCancelledEvent {
  event: "workflowCancelled";
}

export interface RunAllStartedEvent {
  event: "runAllStarted";
  data: { total: number };
}

export interface RunAllSessionStartedEvent {
  event: "runAllSessionStarted";
  data: { sessionId: string; input: string };
}

export interface RunAllSessionFinishedEvent {
  event: "runAllSessionFinished";
  data: { sessionId: string; input: string; phase: SessionPhase; error?: string };
}

export interface RunAllCompletedEvent {
  event: "runAllCompleted";
  data: { cancelled: number };
}

export type WorkflowEvent =
  | StepStartedEvent
  | StepCompletedEvent
  | OptionRequiredEvent
  | WorkflowCompletedEvent
  | WorkflowFailedEvent
  | WorkflowCancelledEvent
  | RunAllStartedEvent
  | RunAllSessionStartedEvent
  | RunAllSessionFinishedEvent
  | RunAllCompletedEvent;

// --- Cleanup ------------------------------------------------------------------

export interface CleanupResult {
  deleted: number;
  skipped: number;
}

// --- Directory listing --------------------------------------------------------

export interface DirEntry {
  name: string;
  path: string;
}

// --- Session creation ---------------------------------------------------------

export interface ConfigEntry {
  path: string;
  name: string;
}

export type PlanEvent =
  | { event: "sessionCreated"; data: { sessionId: string } }
  | { event: "planGenerating"; data: Record<string, never> }
  | { event: "planGenerated"; data: { sessionId: string; content: string } }
  | { event: "planFailed"; data: { sessionId: string; error: string } };

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
