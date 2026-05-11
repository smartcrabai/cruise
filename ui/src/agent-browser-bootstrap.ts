import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

type SessionPhase =
  | "Awaiting Approval"
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
  createdAt: string;
  updatedAt?: string;
  workspaceMode: "Worktree";
  planAvailable?: boolean;
}

interface HistoryEntry {
  selectedAt: string;
  input: string;
  requestedConfigPath?: string;
  workingDir: string;
  resolvedConfigKey: string;
  skippedSteps: string[];
}

const BUILTIN_CONFIG_KEY = "__builtin__";
const TEAM_CONFIG_PATH = "/Users/takumi/.cruise/team.yaml";
const AUTO_CONFIG_PATH = "/Users/takumi/projects/demo/cruise.yaml";

const CONFIG_STEPS: Record<string, string[]> = {
  [BUILTIN_CONFIG_KEY]: ["write-tests", "implement"],
  [TEAM_CONFIG_PATH]: ["research", "write-tests", "implement", "review"],
  [AUTO_CONFIG_PATH]: ["plan", "implement", "verify"],
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

function stepsFor(baseDir: string, configPath?: string | null): string[] {
  return CONFIG_STEPS[resolveConfigKey(baseDir, configPath)] ?? CONFIG_STEPS[BUILTIN_CONFIG_KEY];
}

function latestHistorySummary() {
  const latestGuiEntry = historyEntries.find((entry) => entry.workingDir);
  const recentWorkingDirs: string[] = [];
  const seen = new Set<string>();

  for (const entry of historyEntries) {
    if (entry.workingDir && !seen.has(entry.workingDir)) {
      seen.add(entry.workingDir);
      recentWorkingDirs.push(entry.workingDir);
    }
    if (recentWorkingDirs.length === 5) break;
  }

  return {
    lastRequestedConfigPath: latestGuiEntry?.requestedConfigPath,
    lastWorkingDir: latestGuiEntry?.workingDir,
    recentWorkingDirs,
  };
}

function defaultSkippedSteps(baseDir: string, configPath?: string | null): string[] {
  const resolvedConfigKey = resolveConfigKey(baseDir, configPath);
  const steps = stepsFor(baseDir, configPath);
  const history = historyEntries.find((entry) => entry.resolvedConfigKey === resolvedConfigKey);
  if (!history) return [];
  return steps.filter((step) => history.skippedSteps.includes(step));
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
    case "get_session_log":
      return "";
    case "list_configs":
      return [
        { name: "team.yaml", path: TEAM_CONFIG_PATH },
        { name: "autoflow.yaml", path: "/Users/takumi/.cruise/autoflow.yaml" },
      ];
    case "get_new_session_history_summary":
      return latestHistorySummary();
    case "get_new_session_config_defaults": {
      const baseDir = String(getField(payload, "baseDir") ?? ".");
      const rawConfigPath = getField(payload, "configPath");
      const configPath = rawConfigPath == null ? undefined : String(rawConfigPath);
      return {
        steps: stepsFor(baseDir, configPath),
        defaultSkippedSteps: defaultSkippedSteps(baseDir, configPath),
      };
    }
    case "get_new_session_draft":
      return null;
    case "save_new_session_draft":
      return null;
    case "clear_new_session_draft":
      return null;
    case "list_new_session_history": {
      let limit = Number(getField(payload, "limit") ?? 10);
      if (!Number.isFinite(limit) || limit < 0) {
        limit = 10;
      }
      return historyEntries.slice(0, limit);
    }
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
      const baseDir = String(getField(payload, "baseDir") ?? ".");
      const rawConfigPath = getField(payload, "configPath");
      const configPath = rawConfigPath == null ? undefined : String(rawConfigPath);
      const rawSkippedSteps = getField(payload, "skippedSteps");
      const skippedSteps = Array.isArray(rawSkippedSteps)
        ? rawSkippedSteps.map(String)
        : [];
      const sessionId = `mock-session-${sessions.length + 1}`;
      const resolvedConfigKey = resolveConfigKey(baseDir, configPath);
      const createdAt = new Date().toISOString();

      historyEntries.unshift({
        selectedAt: createdAt,
        input: String(getField(payload, "input") ?? ""),
        requestedConfigPath: configPath,
        workingDir: baseDir,
        resolvedConfigKey,
        skippedSteps,
      });

      sessions.unshift({
        id: sessionId,
        phase: "Awaiting Approval",
        configSource: configPath ? `config: ${configPath}` : "config: (auto)",
        baseDir,
        input: String(getField(payload, "input") ?? ""),
        createdAt,
        updatedAt: createdAt,
        workspaceMode: "Worktree",
        planAvailable: false,
      });

      sessionPlans.set(
        sessionId,
        `# Generated plan\n\n- Working Directory: ${baseDir}\n- Resolved config: ${resolvedConfigKey}`
      );

      emitChannel(getField(payload, "channel"), [
        { event: "sessionCreated", data: { sessionId } },
        { event: "planGenerating", data: {} },
        {
          event: "planGenerated",
          data: { sessionId, content: sessionPlans.get(sessionId) ?? "# Generated plan" },
        },
      ]);

      const session = sessions.find((item) => item.id === sessionId);
      if (session) {
        setTimeout(() => {
          session.planAvailable = true;
        }, 120);
      }
      return sessionId;
    }
    case "get_update_readiness":
      return { canAutoUpdate: true };
    case "get_app_config":
      return { runAllParallelism: 1 };
    case "update_app_config":
    case "clean_sessions":
    case "approve_session":
    case "delete_session":
    case "reset_session":
    case "run_session":
    case "cancel_session":
    case "respond_to_option":
    case "run_all_sessions":
    case "fix_session":
    case "ask_session":
      return null;
    default:
      console.warn("[agent-browser-bootstrap] Unhandled IPC command:", cmd, payload);
      return null;
  }
});

console.info("[agent-browser-bootstrap] mock IPC enabled");
