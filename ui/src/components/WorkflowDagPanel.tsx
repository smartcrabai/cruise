import { useEffect, useRef, useState } from "react";
import type { DagDto, DagEdgeDto } from "../types";
import { getSessionDag } from "../lib/commands";

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
  const [dag, setDag] = useState<DagDto | null>(null);
  const [svg, setSvg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const renderIdRef = useRef(0);

  useEffect(() => {
    let active = true;
    setDag(null);
    setSvg(null);
    setError(null);
    setLoading(true);

    void getSessionDag(sessionId)
      .then((data) => {
        if (active) setDag(data);
      })
      .catch((e) => {
        if (active) setError(String(e));
      })
      .finally(() => {
        if (active) setLoading(false);
      });

    return () => {
      active = false;
    };
  }, [sessionId]);

  useEffect(() => {
    if (!dag) return;

    const myRenderId = ++renderIdRef.current;

    void (async () => {
      try {
        const mermaid = (await import("mermaid")).default;
        mermaid.initialize({ startOnLoad: false, theme: "default" });
        const source = buildMermaidSource(dag);
        const { svg: renderedSvg } = await mermaid.render(
          `dag-${sessionId.replace(/[^a-zA-Z0-9]/g, "-")}-${myRenderId}`,
          source,
        );
        if (myRenderId === renderIdRef.current) {
          setSvg(renderedSvg);
          setError(null);
        }
      } catch (e) {
        if (myRenderId === renderIdRef.current) {
          setError(`Failed to render DAG: ${String(e)}`);
          setSvg(null);
        }
      }
    })();
  }, [dag, sessionId]);

  return (
    <div
      id={panelId}
      role="tabpanel"
      aria-labelledby={tabId}
      className={`h-full overflow-auto ${className}`}
    >
      {error && (
        <p className="p-4 text-sm text-red-600 dark:text-red-400">{error}</p>
      )}
      {loading && !svg && (
        <p className="p-4 text-sm text-gray-500 dark:text-gray-400">
          Loading DAG…
        </p>
      )}
      {!loading && !error && !svg && !dag && (
        <p className="p-4 text-sm text-gray-500 dark:text-gray-400">
          No DAG available.
        </p>
      )}
      {svg && (
        <div
          className="dag-svg p-4"
          // eslint-disable-next-line react/no-danger
          dangerouslySetInnerHTML={{ __html: svg }}
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
  lines.push(`  ${endId}[/END/]`);

  for (const step of dag.steps) {
    const id = stepIdMap.get(step.name);
    if (!id) continue;
    lines.push(`  ${id}["${escapeMermaidLabel(step.name)}"]`);
  }

  for (const edge of dag.edges) {
    const fromId = stepIdMap.get(edge.from);
    if (!fromId) continue;

    const toId = edge.to ? stepIdMap.get(edge.to) : endId;
    if (edge.to && !toId) continue;

    const label = edgeLabel(edge);
    if (label) {
      lines.push(`  ${fromId} -->|"${escapeMermaidLabel(label)}"| ${toId}`);
    } else {
      lines.push(`  ${fromId} --> ${toId}`);
    }
  }

  if (dag.currentStep) {
    const currentId = stepIdMap.get(dag.currentStep);
    if (currentId) {
      lines.push(
        `  style ${currentId} fill:#3b82f6,color:#fff,stroke:#2563eb,stroke-width:2px`,
      );
    }
  }

  const startId = stepIdMap.get(dag.startStep);
  if (startId) {
    lines.push(
      `  style ${startId} fill:#10b981,color:#fff,stroke:#059669,stroke-width:2px`,
    );
  }

  return lines.join("\n");
}

function sanitizeNodeId(name: string): string {
  return name.replace(/[^A-Za-z0-9_]/g, "_");
}

function escapeMermaidLabel(label: string): string {
  return label.replace(/"/g, "#quot;");
}

function edgeLabel(edge: DagEdgeDto): string | null {
  switch (edge.reason) {
    case "sequential":
    case "next":
      return null;
    case "ifFileChanged":
      return edge.to ? `if-file-changed: ${edge.to}` : "if-file-changed";
    case "ifNoFileChangesRetry":
      return "retry (no file changes)";
    case "ifNoFileChangesFail":
      return "fail (no file changes)";
    case "ifFail":
      return edge.to ? `if-fail: ${edge.to}` : "if-fail";
    case "ifFailRetry":
      return "retry (on fail)";
    case "optionChoice":
      return edge.selector ? `opt: ${edge.selector}` : "opt";
    case "groupRetry":
      return edge.to ? `group-retry: ${edge.to}` : "group-retry";
    case "groupRetryExhausted":
      return "group-retry exhausted";
    default:
      return edge.reason;
  }
}
