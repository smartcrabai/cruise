import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { BUILTIN_CONFIG_PATH as BUILTIN_CONFIG_KEY, type SkippableStepDto } from "./types";

type SessionPhase =
  | "Draft"
  | "Awaiting Approval"
  | "Awaiting Input"
  | "Planned"
  | "Running"
  | "Completed"
  | "Failed"
  | "Suspended";

interface MockSession {
  id: string;
  phase: SessionPhase;
  configSource: string;
  baseDir: string;
  input: string;
  workspaceMode: "Worktree" | "CurrentBranch";
  createdAt: string;
  updatedAt?: string;
  planAvailable?: boolean;
  skippedSteps: string[];
}

interface HistoryEntry {
  selectedAt: string;
  input: string;
  requestedConfigPath?: string;
  workingDir: string;
  resolvedConfigKey: string;
  skippedSteps: string[];
}

const TEAM_CONFIG_PATH = "/Users/takumi/.config/cruise/workflows/team.yaml";
const AUTO_CONFIG_PATH = "/Users/takumi/projects/demo/cruise.yaml";

function makeStep(id: string): SkippableStepDto {
  return { id, expandedStepIds: [id], children: [] };
}

function makeGroup(id: string, childIds: string[]): SkippableStepDto {
  return {
    id,
    expandedStepIds: childIds.map((childId) => `${id}/${childId}`),
    children: childIds.map((childId) => makeStep(`${id}/${childId}`)),
  };
}

const CONFIG_STEPS: Record<string, SkippableStepDto[]> = {
  [BUILTIN_CONFIG_KEY]: [
    ...[
      "mise-trust",
      "write-test-first",
      "implement-after-tests",
      "verify-wiring",
      "only-english",
      "simplify-pass",
    ].map(makeStep),
    makeGroup("review-pass", ["review-uncommitted", "fix-review-result"]),
  ],
  [TEAM_CONFIG_PATH]: ["research", "write-tests", "implement", "review"].map(makeStep),
  [AUTO_CONFIG_PATH]: ["plan", "implement", "verify"].map(makeStep),
};

const AFTER_PR_STEPS: Record<string, SkippableStepDto[]> = {
  [BUILTIN_CONFIG_KEY]: [
    "pr-ready",
    "sync-base",
    "resolve-conflict",
    "push-after-sync",
    "wait-ci",
    "fix-ci-error",
    "merge",
  ].map(makeStep),
};

const sessions: MockSession[] = [
  {
    id: "existing-session",
    phase: "Planned",
    configSource: `config: ${TEAM_CONFIG_PATH}`,
    baseDir: "/Users/takumi/projects/demo",
    input: "existing task",
    createdAt: "2026-04-07T10:00:00Z",
    updatedAt: "2026-04-07T10:00:00Z",
    workspaceMode: "Worktree",
    planAvailable: true,
    skippedSteps: [],
  },
];

const sessionPlans = new Map<string, string>([
  [
    "existing-session",
    "# Existing plan\n\n- Verify New Session defaults\n- Keep recent working directories clickable",
  ],
]);

const historyEntries: HistoryEntry[] = [
  {
    selectedAt: "2026-04-07T10:00:00Z",
    input: "fix login bug",
    requestedConfigPath: TEAM_CONFIG_PATH,
    workingDir: "/Users/takumi/projects/demo",
    resolvedConfigKey: TEAM_CONFIG_PATH,
    skippedSteps: ["review"],
  },
  {
    selectedAt: "2026-04-06T10:00:00Z",
    input: "add dark mode",
    workingDir: "/Users/takumi/projects/another-repo",
    resolvedConfigKey: AUTO_CONFIG_PATH,
    skippedSteps: ["verify"],
  },
];

const tauriWindow = window as unknown as Window & {
  __TAURI_INTERNALS__: {
    runCallback: (id: number, payload: unknown) => void;
  };
};

function getField(payload: unknown, key: string): unknown {
  if (payload && typeof payload === "object" && key in payload) {
    return (payload as Record<string, unknown>)[key];
  }
  return undefined;
}

function resolveConfigKey(baseDir: string, configPath?: string | null): string {
  if (configPath) return configPath;
  if (baseDir === "/Users/takumi/projects/demo") return AUTO_CONFIG_PATH;
  return BUILTIN_CONFIG_KEY;
}

function stepsFor(baseDir: string, configPath?: string | null): SkippableStepDto[] {
  return CONFIG_STEPS[resolveConfigKey(baseDir, configPath)] ?? CONFIG_STEPS[BUILTIN_CONFIG_KEY];
}

function afterPrStepsFor(baseDir: string, configPath?: string | null): SkippableStepDto[] {
  return AFTER_PR_STEPS[resolveConfigKey(baseDir, configPath)] ?? [];
}


function defaultSkippedSteps(baseDir: string, configPath?: string | null): string[] {
  const resolvedConfigKey = resolveConfigKey(baseDir, configPath);
  const steps = stepsFor(baseDir, configPath);
  const history = historyEntries.find((entry) => entry.resolvedConfigKey === resolvedConfigKey);
  if (!history) return [];
  return steps
    .flatMap(({ expandedStepIds }) => expandedStepIds)
    .filter((stepId) => history.skippedSteps.includes(stepId));
}

function emitChannel(serializedChannel: unknown, events: unknown[]) {
  const callbackId = (() => {
    if (
      serializedChannel &&
      typeof serializedChannel === "object" &&
      "id" in serializedChannel &&
      typeof serializedChannel.id === "number"
    ) {
      return serializedChannel.id;
    }
    const serialized = String(serializedChannel);
    if (!serialized.startsWith("__CHANNEL__:")) return Number.NaN;
    return Number(serialized.slice("__CHANNEL__:".length));
  })();
  if (Number.isNaN(callbackId)) return;

  events.forEach((event, index) => {
    setTimeout(() => {
      tauriWindow.__TAURI_INTERNALS__.runCallback(callbackId, { index, message: event });
    }, index * 40);
  });

  setTimeout(() => {
    tauriWindow.__TAURI_INTERNALS__.runCallback(callbackId, {
      index: events.length,
      end: true,
    });
  }, events.length * 40);
}

function requestPayload(payload: unknown): Record<string, unknown> {
  const request = getField(payload, "request");
  return request && typeof request === "object" ? request as Record<string, unknown> : {};
}

function sessionDto(session: MockSession): MockSession {
  return { ...session };
}

function emitPlan(session: MockSession, payload: unknown, operation: "generate" | "fix" | "ask" | "replan") {
  const events: unknown[] = [
    { event: "planStarted", data: { sessionId: session.id, operation } },
  ];
  if (operation === "ask") {
    events.push({ event: "planChunk", data: { sessionId: session.id, stream: "stdout", text: "The current plan remains unchanged." } });
  }
  events.push({ event: "planFinished", data: { sessionId: session.id, phase: session.phase } });
  emitChannel(getField(payload, "channel"), events);
}

mockWindows("main");
mockIPC((cmd, payload?: unknown) => {
  switch (cmd) {
    case "plugin:app|version":
      return "0.0.0-agent-browser";
    case "plugin:updater|check":
      return null;
    case "plugin:process|restart":
      return null;
    case "list_sessions":
      return sessions;
    case "get_session": {
      const sessionId = String(getField(payload, "sessionId") ?? "");
      return sessions.find((session) => session.id === sessionId) ?? null;
    }
    case "get_session_plan":
      return sessionPlans.get(String(getField(payload, "sessionId") ?? "")) ?? "# Mock plan";
    case "get_session_dag":
      return null;
    case "get_session_log":
      return "";
    case "list_configs":
      // baseDir and repo are accepted in the payload but ignored in the browser demo;
      // the fixed list simulates user workflow configs only.
      return [
        {
          name: "team.yaml",
          path: TEAM_CONFIG_PATH,
          description: "team-shared: parallel implement + auto-PR",
          source: "user",
        },
        {
          name: "autoflow.yaml",
          path: "/Users/takumi/.config/cruise/workflows/autoflow.yaml",
          source: "user",
        },
      ];
    case "get_new_session_config_defaults": {
      const baseDir = String(getField(payload, "baseDir") ?? ".");
      const rawConfigPath = getField(payload, "configPath");
      const configPath = rawConfigPath == null ? undefined : String(rawConfigPath);
      const resolvedConfigKey = resolveConfigKey(baseDir, configPath);
      return {
        steps: stepsFor(baseDir, configPath),
        afterPrSteps: afterPrStepsFor(baseDir, configPath),
        defaultSkippedSteps: defaultSkippedSteps(baseDir, configPath),
        resolvedConfigKey,
      };
    }
    case "get_new_session_draft":
      return null;
    case "save_new_session_draft":
      return null;
    case "clear_new_session_draft":
      return null;
    case "list_directory": {
      const path = String(getField(payload, "path") ?? "");
      if (path.includes("/Users/takumi/projects")) {
        return [
          { name: "demo", path: "/Users/takumi/projects/demo" },
          { name: "another-repo", path: "/Users/takumi/projects/another-repo" },
        ];
      }
      return [];
    }
    case "create_session": {
      const request = requestPayload(payload);
      const baseDir = String(request.baseDir ?? ".");
      const rawConfigPath = request.configPath;
      const configPath = rawConfigPath == null ? undefined : String(rawConfigPath);
      const rawSkippedSteps = request.skippedSteps;
      const skippedSteps = Array.isArray(rawSkippedSteps) ? rawSkippedSteps.map(String) : [];
      const sessionId = `mock-session-${sessions.length + 1}`;
      const resolvedConfigKey = resolveConfigKey(baseDir, configPath);
      const createdAt = new Date().toISOString();
      const session: MockSession = {
        id: sessionId,
        phase: "Draft",
        configSource: configPath ? `config: ${configPath}` : "config: (auto)",
        baseDir,
        input: String(request.input ?? ""),
        createdAt,
        updatedAt: createdAt,
        workspaceMode: request.workspaceMode === "CurrentBranch" ? "CurrentBranch" : "Worktree",
        planAvailable: false,
        skippedSteps,
      };
      historyEntries.unshift({
        selectedAt: createdAt,
        input: session.input,
        requestedConfigPath: configPath,
        workingDir: baseDir,
        resolvedConfigKey,
        skippedSteps,
      });
      sessions.unshift(session);
      sessionPlans.set(sessionId, `# Generated plan\n\n- Working Directory: ${baseDir}\n- Resolved config: ${resolvedConfigKey}`);
      return sessionDto(session);
    }
    case "use_input_as_plan":
    case "generate_plan_for_draft":
    case "regenerate_session_plan":
    case "fix_session": {
      const sessionId = String(getField(payload, "sessionId") ?? "");
      const session = sessions.find((item) => item.id === sessionId);
      if (!session) return null;
      session.phase = "Awaiting Approval";
      session.planAvailable = true;
      session.updatedAt = new Date().toISOString();
      const operation = cmd === "fix_session" ? "fix" : cmd === "regenerate_session_plan" ? "replan" : "generate";
      emitPlan(session, payload, operation);
      return sessionDto(session);
    }
    case "ask_session": {
      const sessionId = String(getField(payload, "sessionId") ?? "");
      const session = sessions.find((item) => item.id === sessionId);
      if (!session) return null;
      emitPlan(session, payload, "ask");
      return sessionDto(session);
    }
    case "get_update_readiness":
      return { canAutoUpdate: true };
    case "get_app_config":
      return { runAllParallelism: 1 };
    case "update_app_config":
      return null;
    case "clean_sessions":
      return { deleted: 0, skipped: sessions.length, noPrDeleted: 0 };
    case "approve_session": {
      const session = sessions.find((item) => item.id === String(getField(payload, "sessionId") ?? ""));
      if (!session) return null;
      session.phase = "Planned";
      return sessionDto(session);
    }
    case "delete_session":
    case "discard_session": {
      const sessionId = String(getField(payload, "sessionId") ?? "");
      const index = sessions.findIndex((session) => session.id === sessionId);
      if (index >= 0) sessions.splice(index, 1);
      return null;
    }
    case "reset_session": {
      const session = sessions.find((item) => item.id === String(getField(payload, "sessionId") ?? ""));
      if (!session) return null;
      session.phase = "Planned";
      return sessionDto(session);
    }
    case "update_session": {
      const session = sessions.find((item) => item.id === String(getField(payload, "sessionId") ?? ""));
      return session ? sessionDto(session) : null;
    }
    case "publish_plan_issue": {
      const sessionId = String(getField(payload, "sessionId") ?? "");
      const index = sessions.findIndex((session) => session.id === sessionId);
      if (index < 0) return null;
      sessions.splice(index, 1);
      return { url: `https://github.com/owner/repo/issues/${index + 1}`, repo: "owner/repo" };
    }
    case "pending_prompts":
      return [];
    case "run_session": {
      const sessionId = String(getField(payload, "sessionId") ?? "");
      const session = sessions.find((item) => item.id === sessionId);
      if (!session) return null;
      session.phase = "Running";
      emitChannel(getField(payload, "channel"), [
        { event: "runStarted", data: { sessionId } },
        { event: "stepStarted", data: { sessionId, step: "verify-wiring" } },
        { event: "logChunk", data: { sessionId, stream: "info", text: "Mock run", batch: false } },
        { event: "runFinished", data: { sessionId, phase: "Completed" } },
      ]);
      session.phase = "Completed";
      session.planAvailable = true;
      return sessionDto(session);
    }
    case "cancel_session":
    case "cancel_run_all":
      return false;
    case "respond_to_ask":
      return null;
    case "respond_to_option":
      return null;
    case "run_all_sessions": {
      const runnable = sessions.filter((session) => session.phase === "Planned" || session.phase === "Suspended");
      const channel = getField(payload, "channel");
      emitChannel(channel, [
        { event: "batchStarted", data: { total: runnable.length, parallelism: 1 } },
        ...runnable.flatMap((session) => [
          { event: "batchSessionStarted", data: { id: session.id } },
          { event: "batchSessionFinished", data: { id: session.id, phase: "Completed", error: null } },
        ]),
        { event: "batchFinished", data: { cancelled: false } },
      ]);
      return null;
    }
    default:
      console.warn("[agent-browser-bootstrap] Unhandled IPC command:", cmd, payload);
      return null;
    }
});

console.info("[agent-browser-bootstrap] mock IPC enabled");
