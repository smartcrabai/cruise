import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  AppConfig,
  CleanupResult,
  ConfigEntry,
  DirEntry,
  PlanEvent,
  Session,
  UpdateReadiness,
  WorkspaceMode,
  WorkflowEvent,
} from "../types";

/** List all sessions, sorted oldest-first. */
export function listSessions(): Promise<Session[]> {
  return invoke<Session[]>("list_sessions");
}

/** Get a single session by ID. */
export function getSession(sessionId: string): Promise<Session> {
  return invoke<Session>("get_session", { sessionId });
}

/** Return the plan markdown for a session. */
export function getSessionPlan(sessionId: string): Promise<string> {
  return invoke<string>("get_session_plan", { sessionId });
}

/**
 * Run a session's workflow, streaming events via the returned Channel.
 *
 * @example
 * const channel = new Channel<WorkflowEvent>();
 * channel.onmessage = (event) => { ... };
 * await runSession(sessionId, "Worktree", channel);
 */
export function runSession(
  sessionId: string,
  workspaceMode: WorkspaceMode,
  channel: Channel<WorkflowEvent>
): Promise<void> {
  return invoke<void>("run_session", { sessionId, workspaceMode, channel });
}

/** Cancel the currently running workflow. */
export function cancelSession(): Promise<void> {
  return invoke<void>("cancel_session");
}

/**
 * Run all Planned / Suspended sessions in series, streaming events via `channel`.
 */
export function runAllSessions(
  channel: Channel<WorkflowEvent>
): Promise<void> {
  return invoke<void>("run_all_sessions", { channel });
}

/**
 * Respond to a pending option-step request.
 *
 * @param result.nextStep  The next step to jump to (selector choice).
 * @param result.textInput Free-text input (text-input choice).
 */
export function respondToOption(
  sessionId: string,
  result: { nextStep?: string; textInput?: string },
): Promise<void> {
  return invoke<void>("respond_to_option", { sessionId, result });
}

// ─── App config ───────────────────────────────────────────────────────────────

/**
 * Retrieve the current application configuration from `~/.config/cruise/config.json`.
 *
 * If the file does not exist the server returns the default config (`runAllParallelism: 1`).
 */
export function getAppConfig(): Promise<AppConfig> {
  return invoke<AppConfig>("get_app_config");
}

/**
 * Persist an updated application configuration.
 *
 * The server validates the config before writing (e.g. `runAllParallelism` must be ≥ 1).
 * Throws if validation fails or the file cannot be written.
 */
export function updateAppConfig(config: AppConfig): Promise<void> {
  return invoke<void>("update_app_config", { config });
}

/** Remove Completed sessions whose PR is closed or merged. */
export function cleanSessions(): Promise<CleanupResult> {
  return invoke<CleanupResult>("clean_sessions");
}

/** Reset a session to "Planned" phase regardless of its current phase. */
export function resetSession(sessionId: string): Promise<Session> {
  return invoke<Session>("reset_session", { sessionId });
}

/** Return the run log for a session as plain text. Empty string if not yet run. */
export function getSessionLog(sessionId: string): Promise<string> {
  return invoke<string>("get_session_log", { sessionId });
}

// ─── Filesystem ───────────────────────────────────────────────────────────────

/** Return whether the current launch context supports automatic in-place update. */
export function getUpdateReadiness(): Promise<UpdateReadiness> {
  return invoke<UpdateReadiness>("get_update_readiness");
}

/** List subdirectories of `path`. `~` is expanded server-side. Returns up to 50 entries. */
export function listDirectory(path: string): Promise<DirEntry[]> {
  return invoke<DirEntry[]>("list_directory", { path });
}

// ─── Session creation ─────────────────────────────────────────────────────────

/** List workflow config files in ~/.cruise/. */
export function listConfigs(): Promise<ConfigEntry[]> {
  return invoke<ConfigEntry[]>("list_configs");
}

/** Return step names defined in a workflow config file. */
export function getConfigSteps(configPath?: string): Promise<string[]> {
  return invoke<string[]>("get_config_steps", {
    configPath: configPath ?? null,
  });
}

/**
 * Create a new session and generate a plan, streaming PlanEvents via `channel`.
 *
 * @returns The new session ID.
 */
export function createSession(
  params: { input: string; configPath?: string; baseDir: string; skippedSteps?: string[] },
  channel: Channel<PlanEvent>
): Promise<string> {
  return invoke<string>("create_session", {
    input: params.input,
    configPath: params.configPath ?? null,
    baseDir: params.baseDir,
    skippedSteps: params.skippedSteps ?? [],
    channel,
  });
}

/** Approve a session (Awaiting Approval → Planned). */
export function approveSession(sessionId: string): Promise<void> {
  return invoke<void>("approve_session", { sessionId });
}

/**
 * Ask a question about a session's plan without modifying it.
 *
 * @returns The LLM answer text (transient; not persisted to plan.md).
 */
export function askSession(sessionId: string, question: string): Promise<string> {
  return invoke<string>("ask_session", { sessionId, question });
}

/** Delete a session that is still awaiting approval. */
export function discardSession(sessionId: string): Promise<void> {
  return invoke<void>("discard_session", { sessionId });
}

/** Delete a session and clean up its worktree. Cannot delete Running sessions. */
export function deleteSession(sessionId: string): Promise<void> {
  return invoke<void>("delete_session", { sessionId });
}

/**
 * Re-generate the plan for an existing session with the given feedback,
 * streaming PlanEvents via `channel`.
 *
 * @returns The updated plan markdown.
 */
export function fixSession(
  params: { sessionId: string; feedback: string },
  channel: Channel<PlanEvent>
): Promise<string> {
  return invoke<string>("fix_session", {
    sessionId: params.sessionId,
    feedback: params.feedback,
    channel,
  });
}
