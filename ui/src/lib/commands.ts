import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  ApplicationEvent,
  AppConfig,
  CleanupResult,
  ConfigEntry,
  DagDto,
  DirEntry,
  NewSessionConfigDefaults,
  NewSessionDraftPersisted,
  NewSessionHistorySummary,
  PendingPrompt,
  PublishedIssue,
  Session,
  UpdateReadiness,
  WorkspaceMode,
} from "../types";

export type EventChannel = Channel<ApplicationEvent>;

export function listSessions(): Promise<Session[]> {
  return invoke<Session[]>("list_sessions");
}

export function getSession(sessionId: string): Promise<Session> {
  return invoke<Session>("get_session", { sessionId });
}

export function getSessionPlan(sessionId: string): Promise<string> {
  return invoke<string>("get_session_plan", { sessionId });
}
export function getSessionLog(sessionId: string): Promise<string> {
  return invoke<string>("get_session_log", { sessionId });
}


export function getSessionDag(sessionId: string): Promise<DagDto> {
  return invoke<DagDto | null>("get_session_dag", { sessionId }).then(
    (dag) => dag ?? { startStep: "", steps: [], edges: [], currentStep: null },
  );
}

function runRequest(workspaceMode?: WorkspaceMode) {
  return { workspaceMode: workspaceMode ?? null, maxRetries: null, rateLimitRetries: 5 };
}

export function runSession(
  sessionId: string,
  workspaceMode: WorkspaceMode,
  channel: EventChannel,
): Promise<Session> {
  return invoke<Session>("run_session", {
    sessionId,
    request: runRequest(workspaceMode),
    channel,
  });
}

export function cancelSession(sessionId: string): Promise<boolean> {
  return invoke<boolean>("cancel_session", { sessionId });
}

export function cancelRunAll(): Promise<boolean> {
  return invoke<boolean>("cancel_run_all");
}

export function runAllSessions(
  channel: EventChannel,
  parallelism?: number,
): Promise<void> {
  return invoke<void>("run_all_sessions", {
    parallelism: parallelism ?? null,
    channel,
  });
}

export function respondToOption(
  sessionId: string,
  requestId: string,
  result: { nextStep?: string; textInput?: string },
): Promise<void> {
  return invoke<void>("respond_to_option", { sessionId, requestId, result });
}

export function respondToAsk(
  sessionId: string,
  requestId: string,
  answer: string,
): Promise<void> {
  return invoke<void>("respond_to_ask", { sessionId, requestId, answer });
}
export function getPendingPrompts(sessionId: string): Promise<PendingPrompt[]> {
  return invoke<PendingPrompt[]>("pending_prompts", { sessionId });
}

export function getAppConfig(): Promise<AppConfig> {
  return invoke<AppConfig>("get_app_config");
}

export function updateAppConfig(config: AppConfig): Promise<void> {
  return invoke<void>("update_app_config", { config });
}

export function cleanSessions(): Promise<CleanupResult> {
  return invoke<CleanupResult>("clean_sessions");
}

export function resetSession(sessionId: string): Promise<Session> {
  return invoke<Session>("reset_session", { sessionId });
}

function currentStepUpdate(currentStep: string | null | undefined) {
  if (currentStep === undefined) return "unchanged";
  return currentStep === null ? "clear" : { set: currentStep };
}

export function updateSessionSettings(
  sessionId: string,
  params: { configPath?: string; skippedSteps: string[]; currentStep?: string | null },
): Promise<Session> {
  const configPath = params.configPath === undefined ? {} : { configPath: params.configPath };
  return invoke<Session>("update_session", {
    sessionId,
    request: {
      ...configPath,
      skippedSteps: params.skippedSteps,
      currentStepUpdate: currentStepUpdate(params.currentStep),
    },
  });
}


export function regenerateSessionPlan(
  sessionId: string,
  channel: EventChannel,
  feedback?: string,
): Promise<string> {
  return invoke<Session>("regenerate_session_plan", {
    sessionId,
    request: {
      grill: false,
      skipPlanning: false,
      noInteractivePlanning: false,
      interactive: true,
      rateLimitRetries: 5,
      feedback: feedback ?? null,
      question: null,
    },
    channel,
  }).then(() => getSessionPlan(sessionId));
}
export function getUpdateReadiness(): Promise<UpdateReadiness> {
  return invoke<UpdateReadiness>("get_update_readiness");
}

export function listDirectory(path: string): Promise<DirEntry[]> {
  return invoke<DirEntry[]>("list_directory", { path });
}

export function listGithubRepos(): Promise<string[]> {
  return invoke<string[]>("list_github_repos");
}

export function listConfigs(params?: { baseDir?: string; repo?: string }): Promise<ConfigEntry[]> {
  return invoke<ConfigEntry[]>("list_configs", {
    baseDir: params?.baseDir ?? null,
    repo: params?.repo ?? null,
  });
}

export function getNewSessionHistorySummary(): Promise<NewSessionHistorySummary> {
  return invoke<NewSessionHistorySummary>("get_new_session_history_summary");
}

export function getNewSessionConfigDefaults(
  params: { baseDir: string; configPath?: string; repo?: string },
): Promise<NewSessionConfigDefaults> {
  return invoke<NewSessionConfigDefaults>("get_new_session_config_defaults", {
    baseDir: params.baseDir,
    configPath: params.configPath ?? null,
    repo: params.repo ?? null,
  });
}

export function getNewSessionDraft(): Promise<NewSessionDraftPersisted | null> {
  return invoke<NewSessionDraftPersisted | null>("get_new_session_draft");
}

export function saveNewSessionDraft(draft: NewSessionDraftPersisted): Promise<void> {
  return invoke<void>("save_new_session_draft", { draft });
}

export function clearNewSessionDraft(): Promise<void> {
  return invoke<void>("clear_new_session_draft");
}

type NewSessionParams = {
  input: string;
  configPath?: string;
  baseDir: string;
  repo?: string;
  skippedSteps?: string[];
  useInputAsPlan?: boolean;
  grill?: boolean;
  noInteractivePlanning?: boolean;
  imageAttachments?: string[];
  workspaceMode?: WorkspaceMode;
  allowDirtyWorkingTree?: boolean;
};

function newSessionRequest(params: NewSessionParams) {
  const configPath = params.configPath === undefined ? {} : { configPath: params.configPath };
  return {
    input: params.input,
    baseDir: params.baseDir,
    ...configPath,
    configSource: null,
    configYaml: null,
    repo: params.repo ?? null,
    workspaceMode: params.workspaceMode ?? "Worktree",
    allowDirtyWorkingTree: params.allowDirtyWorkingTree ?? false,
    attachments: params.imageAttachments ?? [],
    skippedSteps: params.skippedSteps ?? [],
  };
}

const planRequest = (params: NewSessionParams) => ({
  grill: params.grill ?? false,
  // skipPlanning is reserved for the explicit "use input as plan" flow.
  // Non-interactive planning still invokes the LLM, only without planning tools.
  skipPlanning: false,
  noInteractivePlanning: params.noInteractivePlanning ?? false,
  // `interactive` controls clarification prompts, independently of planning tools.
  interactive: true,
  rateLimitRetries: 5,
  feedback: null,
  question: null,
});

export async function createSession(
  params: NewSessionParams,
  channel: EventChannel,
): Promise<string> {
  const session = await invoke<Session>("create_session", { request: newSessionRequest(params) });
  if (params.useInputAsPlan) {
    await invoke<Session>("use_input_as_plan", { sessionId: session.id, channel });
  } else {
    await invoke<Session>("generate_plan_for_draft", {
      sessionId: session.id,
      request: planRequest(params),
      channel,
    });
  }
  return session.id;
}

export function createDraftSession(params: NewSessionParams): Promise<string> {
  return invoke<Session>("create_session", { request: newSessionRequest(params) }).then((session) => session.id);
}

export function approveSession(sessionId: string): Promise<void> {
  return invoke<Session>("approve_session", { sessionId }).then(() => undefined);
}

export function publishPlanIssue(sessionId: string, triggerCruise: boolean): Promise<PublishedIssue> {
  return invoke<PublishedIssue>("publish_plan_issue", { sessionId, triggerCruise });
}

export function generatePlanForDraft(
  sessionId: string,
  channel: EventChannel,
  request: Partial<{
    grill: boolean;
    noInteractivePlanning: boolean;
    interactive: boolean;
    rateLimitRetries: number;
    feedback: string;
    question: string;
  }> = {},
): Promise<string> {
  return invoke<Session>("generate_plan_for_draft", {
    sessionId,
    request: {
      grill: request.grill ?? false,
      skipPlanning: false,
      noInteractivePlanning: request.noInteractivePlanning ?? false,
      interactive: request.interactive ?? true,
      rateLimitRetries: request.rateLimitRetries ?? 5,
      feedback: request.feedback ?? null,
      question: request.question ?? null,
    },
    channel,
  }).then(() => getSessionPlan(sessionId));
}

export function askSession(
  sessionId: string,
  question: string,
  channel: EventChannel,
): Promise<void> {
  return invoke<Session>("ask_session", { sessionId, question, channel }).then(() => undefined);
}

export function discardSession(sessionId: string): Promise<void> {
  return invoke<void>("discard_session", { sessionId });
}

export function deleteSession(sessionId: string): Promise<void> {
  return invoke<void>("delete_session", { sessionId });
}

export function fixSession(
  params: { sessionId: string; feedback: string },
  channel: EventChannel,
): Promise<string> {
  return invoke<Session>("fix_session", {
    sessionId: params.sessionId,
    feedback: params.feedback,
    channel,
  }).then(() => getSessionPlan(params.sessionId));
}

