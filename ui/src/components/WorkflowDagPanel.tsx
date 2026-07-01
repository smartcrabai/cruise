import { useEffect, useRef, useState } from "react";
import type { DagDto, DagEdgeDto } from "../types";
import { getSessionDag } from "../lib/commands";

// Initialize Mermaid once at module load time; calling initialize() on every
// render would reset global config and could break concurrent renders.
let mermaidInitialized = false;

type DagPanelState =
  | { kind: "loading" }
  | { kind: "error"; message: string }
  | { kind: "svg"; svg: string }
  | { kind: "empty" };

interface WorkflowDagPanelProps {
  sessionId: string;
  panelId: string;
  tabId: string;
  className?: string;
}

export function WorkflowDagPanel({
  sessionId,
  panelId,
  tabId,
  className = "",
}: WorkflowDagPanelProps) {
  const [state, setState] = useState<DagPanelState>({ kind: "loading" });
  const renderIdRef = useRef(0);

  useEffect(() => {
    let active = true;
    const myRenderId = ++renderIdRef.current;
    setState({ kind: "loading" });

    void (async () => {
      try {
        const data = await getSessionDag(sessionId);
        if (!active) return;

        if (data.steps.length === 0) {
          setState({ kind: "empty" });
          return;
        }

        const mermaid = (await import("mermaid")).default;
        if (!mermaidInitialized) {
          mermaid.initialize({ startOnLoad: false, theme: "default" });
          mermaidInitialized = true;
        }
        const source = buildMermaidSource(data);
        const { svg: renderedSvg } = await mermaid.render(
          `dag-${sessionId.replace(/[^a-zA-Z0-9]/g, "-")}-${myRenderId}`,
          source,
        );
        if (active && myRenderId === renderIdRef.current) {
          setState({ kind: "svg", svg: renderedSvg });
        }
      } catch (e) {
        if (active && myRenderId === renderIdRef.current) {
          setState({
            kind: "error",
            message: `Failed to render DAG: ${String(e)}`,
          });
        }
      }
    })();

    return () => {
      active = false;
    };
  }, [sessionId]);

  return (
    <div
      id={panelId}
      role="tabpanel"
      aria-labelledby={tabId}
      className={`h-full overflow-auto ${className}`}
    >
      {state.kind === "error" && (
        <p className="p-4 text-sm text-red-600 dark:text-red-400">
          {state.message}
        </p>
      )}
      {state.kind === "loading" && (
        <p className="p-4 text-sm text-gray-500 dark:text-gray-400">
          Loading DAG…
        </p>
      )}
      {state.kind === "empty" && (
        <p className="p-4 text-sm text-gray-500 dark:text-gray-400">
          No DAG available.
        </p>
      )}
      {state.kind === "svg" && (
        <div
          className="dag-svg p-4"
          // eslint-disable-next-line react/no-danger
          dangerouslySetInnerHTML={{ __html: state.svg }}
        />
      )}
    </div>
  );
}

function buildMermaidSource(dag: DagDto): string {
  const lines: string[] = ["graph TD"];

  const stepIdMap = new Map<string, string>();
  dag.steps.forEach((step, index) => {
    const sanitized = sanitizeNodeId(step.name);
    stepIdMap.set(step.name, `s${index}_${sanitized}`);
  });

  const endId = "end_terminal";
  const hasTerminalEdge = dag.edges.some((e) => e.to === null);
  if (hasTerminalEdge) {
    lines.push(`  ${endId}[/END/]`);
  }

  for (const step of dag.steps) {
    const id = stepIdMap.get(step.name);
    if (!id) continue;
    lines.push(`  ${id}["${escapeMermaidLabel(step.name)}"]`);
  }

  for (const edge of dag.edges) {
    const fromId = stepIdMap.get(edge.from);
    if (!fromId) continue;

    const toId = edge.to === null ? endId : stepIdMap.get(edge.to);
    if (edge.to !== null && !toId) continue;

    const label = edgeLabel(edge);
    if (label) {
      lines.push(`  ${fromId} -->|"${escapeMermaidLabel(label)}"| ${toId}`);
    } else {
      lines.push(`  ${fromId} --> ${toId}`);
    }
  }

  // startStep style first so currentStep style takes precedence when they coincide.
  const startId = stepIdMap.get(dag.startStep);
  if (startId) {
    lines.push(
      `  style ${startId} fill:#10b981,color:#fff,stroke:#059669,stroke-width:2px`,
    );
  }

  if (dag.currentStep) {
    const currentId = stepIdMap.get(dag.currentStep);
    if (currentId) {
      lines.push(
        `  style ${currentId} fill:#3b82f6,color:#fff,stroke:#2563eb,stroke-width:2px`,
      );
    }
  }

  return lines.join("\n");
}

function sanitizeNodeId(name: string): string {
  return name.replace(/[^A-Za-z0-9_]/g, "_");
}

const MERMAID_LABEL_ESCAPES: Record<string, string> = {
  "\\": "#92;",
  "\n": "#10;",
  "\r": "#10;",
  "#": "#35;",
  ";": "#59;",
  "`": "#96;",
  "[": "#91;",
  "]": "#93;",
  "(": "#40;",
  ")": "#41;",
  "{": "#123;",
  "}": "#125;",
  "&": "#amp;",
  '"': "#quot;",
  "<": "#lt;",
  ">": "#gt;",
  "|": "#124;",
};

function escapeMermaidLabel(label: string): string {
  // Single pass over a character class + callback so entity strings we
  // insert (which themselves contain "#" and ";") are never rescanned.
  return label.replace(
    /[\\\n\r#;`[\](){}&"<>|]/g,
    (ch) => MERMAID_LABEL_ESCAPES[ch] ?? ch,
  );
}

function edgeLabel(edge: DagEdgeDto): string | null {
  switch (edge.reason) {
    case "sequential":
    case "next":
      return null;
    case "ifFileChanged":
      return edge.selector ? `if-file-changed: ${edge.selector}` : "if-file-changed";
    case "ifNoFileChangesRetry":
      return "retry (no file changes)";
    case "ifNoFileChangesFail":
      return "fail (no file changes)";
    case "ifFail":
      return edge.selector ? `if-fail: ${edge.selector}` : "if-fail";
    case "ifFailRetry":
      return "retry (on fail)";
    case "optionChoice":
      return edge.selector ? `opt: ${edge.selector}` : "opt";
    case "groupRetry":
      return edge.selector ? `group-retry: ${edge.selector}` : "group-retry";
    case "groupRetryExhausted":
      return "group-retry exhausted";
  }
}
