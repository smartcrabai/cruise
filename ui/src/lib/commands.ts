import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  CleanupResult,
  ConfigEntry,
  DirEntry,
  PlanEvent,
  Session,
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
 * await runSession(sessionId, channel);
 */
export function runSession(
  sessionId: string,
  channel: Channel<WorkflowEvent>
): Promise<void> {
  return invoke<void>("run_session", { sessionId, channel });
}

/** Cancel the currently running workflow. */
export function cancelSession(): Promise<void> {
  return invoke<void>("cancel_session");
}

/**
 * Respond to a pending option-step request.
 *
 * @param result.nextStep  The next step to jump to (selector choice).
 * @param result.textInput Free-text input (text-input choice).
 */
export function respondToOption(result: {
  nextStep?: string;
  textInput?: string;
}): Promise<void> {
  return invoke<void>("respond_to_option", { result });
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

/** List subdirectories of `path`. `~` is expanded server-side. Returns up to 50 entries. */
export function listDirectory(path: string): Promise<DirEntry[]> {
  return invoke<DirEntry[]>("list_directory", { path });
}

// ─── Session creation ─────────────────────────────────────────────────────────

/** List workflow config files in ~/.cruise/. */
export function listConfigs(): Promise<ConfigEntry[]> {
  return invoke<ConfigEntry[]>("list_configs");
}

/**
 * Create a new session and generate a plan, streaming PlanEvents via `channel`.
 *
 * @returns The new session ID.
 */
export function createSession(
  params: { input: string; configPath?: string; baseDir: string },
  channel: Channel<PlanEvent>
): Promise<string> {
  return invoke<string>("create_session", {
    input: params.input,
    configPath: params.configPath ?? null,
    baseDir: params.baseDir,
    channel,
  });
}

/** Approve a session (Awaiting Approval → Planned). */
export function approveSession(sessionId: string): Promise<void> {
  return invoke<void>("approve_session", { sessionId });
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
