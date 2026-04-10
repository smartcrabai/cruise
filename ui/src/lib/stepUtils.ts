import type { SkippableStepDto } from "../types";

export function collectExpandedStepIds(nodes: SkippableStepDto[]): Set<string> {
  const ids = new Set<string>();

  function visit(node: SkippableStepDto) {
    for (const id of node.expandedStepIds) {
      ids.add(id);
    }
    for (const child of node.children) {
      visit(child);
    }
  }

  for (const node of nodes) {
    visit(node);
  }

  return ids;
}
