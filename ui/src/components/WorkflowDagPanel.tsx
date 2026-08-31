import { useEffect, useState } from "react";
import type { DagDto } from "../types";
import { getSessionDag } from "../lib/commands";

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

export function WorkflowDagPanel({ sessionId, panelId, tabId, className = "" }: WorkflowDagPanelProps) {
  const [state, setState] = useState<DagPanelState>({ kind: "loading" });

  useEffect(() => {
    let active = true;
    const renderId = crypto.randomUUID();
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
        const { svg } = await mermaid.render(`dag-${renderId}`, buildMermaidSource(data));
        if (active) setState({ kind: "svg", svg });
      } catch (e) {
        if (active) setState({ kind: "error", message: `Failed to render DAG: ${String(e)}` });
      }
    })();
    return () => { active = false; };
  }, [sessionId]);

  return (
    <div id={panelId} role="tabpanel" aria-labelledby={tabId} className={`h-full overflow-auto ${className}`}>
      {state.kind === "error" && <p className="p-4 text-sm text-red-600 dark:text-red-400">{state.message}</p>}
      {state.kind === "loading" && <p className="p-4 text-sm text-gray-500 dark:text-gray-400">Loading DAG…</p>}
      {state.kind === "empty" && <p className="p-4 text-sm text-gray-500 dark:text-gray-400">No DAG available.</p>}
      {state.kind === "svg" && <div className="dag-svg p-4" dangerouslySetInnerHTML={{ __html: state.svg }} />}
    </div>
  );
}


function buildMermaidSource(dag: DagDto): string {
  const lines: string[] = ["graph TD"];
  const nodeId = new Map(dag.steps.map((step, index) => [step.name, `s${index}_${sanitizeNodeId(step.name)}`]));
  for (const step of dag.steps) lines.push(`  ${nodeId.get(step.name)}["${escapeMermaidLabel(step.name)}"]`);
  for (const edge of dag.edges) {
    const from = nodeId.get(edge.from);
    if (!from) continue;
    if (!edge.to) {
      lines.push(`  ${from} --> end_terminal[/END/]`);
      continue;
    }
    const to = nodeId.get(edge.to);
    if (!to) continue;
    const reason = edge.selector ? { [edge.reason]: edge.selector } : edge.reason;
    const label = edgeLabel(reason);
    lines.push(label ? `  ${from} -->|"${escapeMermaidLabel(label)}"| ${to}` : `  ${from} --> ${to}`);
  }
  const start = nodeId.get(dag.startStep);
  if (start) lines.push(`  style ${start} fill:#10b981,color:#fff,stroke:#059669,stroke-width:2px`);
  const current = nodeId.get(dag.currentStep ?? "");
  if (current) lines.push(`  style ${current} fill:#3b82f6,color:#fff,stroke:#2563eb,stroke-width:2px`);
  return lines.join("\n");
}


function sanitizeNodeId(name: string): string { return name.replace(/[^A-Za-z0-9_]/g, "_"); }

const MERMAID_LABEL_ESCAPES: Record<string, string> = {
  "\\": "#92;", "\n": "#10;", "\r": "#10;", "#": "#35;", ";": "#59;", "`": "#96;",
  "[": "#91;", "]": "#93;", "(": "#40;", ")": "#41;", "{": "#123;", "}": "#125;",
  "&": "#amp;", '"': "#quot;", "<": "#lt;", ">": "#gt;", "|": "#124;",
};

function escapeMermaidLabel(label: string): string {
  return label.replace(/[\\\n\r#;`[\](){}&"<>|]/g, (ch) => MERMAID_LABEL_ESCAPES[ch] ?? ch);
}

function edgeLabel(reason: Record<string, string> | string): string | null {
  if (typeof reason === "string") {
    const normalized = reason.toLowerCase();
    return normalized === "sequential" || normalized === "next" ? null : reason;
  }
  const [kind, value] = Object.entries(reason)[0] ?? [];
  if (!kind || kind.toLowerCase() === "sequential" || kind.toLowerCase() === "next") return null;
  return value ? `${kind}: ${value}` : kind;
}
