import { render, screen, cleanup, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Session, SkippableStepDto } from "../types";

vi.mock("@tauri-apps/api/core", () => ({
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
  },
}));

vi.mock("../lib/commands", () => ({
  listConfigs: vi.fn(),
  getNewSessionConfigDefaults: vi.fn(),
  updateSessionSettings: vi.fn(),
  regenerateSessionPlan: vi.fn(),
}));

import { listConfigs, getNewSessionConfigDefaults, updateSessionSettings, regenerateSessionPlan } from "../lib/commands";
import { SessionConfigEditor } from "./SessionConfigEditor";

const mockListConfigs = vi.mocked(listConfigs);
const mockGetDefaults = vi.mocked(getNewSessionConfigDefaults);
const mockUpdateSettings = vi.mocked(updateSessionSettings);
const mockRegenerate = vi.mocked(regenerateSessionPlan);

function makeSession(overrides: Partial<Session> = {}): Session {
  return {
    id: "session-1",
    phase: "Planned",
    configSource: "default.yaml",
    baseDir: "/home/user/project",
    input: "test task",
    createdAt: "2026-01-01T00:00:00Z",
    workspaceMode: "Worktree",
    skippedSteps: [],
    ...overrides,
  };
}

function makeStep(id: string, children: SkippableStepDto[] = []): SkippableStepDto {
  const expandedStepIds = children.length === 0
    ? [id]
    : children.flatMap((c) => c.expandedStepIds);
  return { id, expandedStepIds, children };
}

const defaultProps = {
  sessionId: "session-1",
  baseDir: "/home/user/project",
  configPath: undefined,
  skippedSteps: [] as string[],
  onSessionUpdated: vi.fn(),
  onPlanRegenerated: vi.fn(),
  onRegeneratingChange: vi.fn(),
  onError: vi.fn(),
  disabled: false,
};

beforeEach(() => {
  vi.clearAllMocks();
  mockListConfigs.mockResolvedValue([]);
  mockGetDefaults.mockResolvedValue({ steps: [] });
  mockUpdateSettings.mockResolvedValue(makeSession());
  mockRegenerate.mockResolvedValue(undefined);
});

afterEach(() => cleanup());

describe("SessionConfigEditor", () => {
  describe("Save button visibility control", () => {
    it("Save button is not shown when there are no changes", async () => {
      // Given
      render(<SessionConfigEditor {...defaultProps} />);
      // When (no changes)
      // Then
      await waitFor(() => {
        expect(screen.queryByRole("button", { name: /save/i })).not.toBeInTheDocument();
        expect(screen.queryByRole("button", { name: /regenerate/i })).not.toBeInTheDocument();
      });
    });

    it("'Save' button is shown when skip steps are changed", async () => {
      // Given
      const steps = [makeStep("step-a"), makeStep("step-b")];
      mockGetDefaults.mockResolvedValue({ steps });
      render(<SessionConfigEditor {...defaultProps} />);
      await waitFor(() => screen.getByLabelText("step-a"));

      // When: check step-a checkbox
      await userEvent.click(screen.getByLabelText("step-a"));

      // Then
      expect(screen.getByRole("button", { name: /^save$/i })).toBeInTheDocument();
    });

    it("'Save & Regenerate Plan' button is shown when config is changed", async () => {
      // Given
      mockListConfigs.mockResolvedValue([{ name: "custom.yaml", path: "/path/custom.yaml" }]);
      render(<SessionConfigEditor {...defaultProps} />);
      await waitFor(() => screen.getByLabelText("Config"));

      // When: change config
      await userEvent.selectOptions(screen.getByLabelText("Config"), "/path/custom.yaml");

      // Then
      await waitFor(() => {
        expect(screen.getByRole("button", { name: /save & regenerate plan/i })).toBeInTheDocument();
      });
    });
  });

  describe("Save button disabled state", () => {
    it("Save button is disabled when disabled=true", async () => {
      // Given: re-render with disabled=true after changing skip steps,
      // to simulate state where there are changes but disabled=true
      const steps = [makeStep("step-a")];
      mockGetDefaults.mockResolvedValue({ steps });
      const { rerender } = render(
        <SessionConfigEditor {...defaultProps} skippedSteps={["step-a"]} />
      );
      await waitFor(() => screen.getByLabelText("step-a"));

      // When: uncheck to create a change, then re-render with disabled=true
      // skippedSteps stays ["step-a"] so hasSkipChanged remains true after the uncheck
      await userEvent.click(screen.getByLabelText("step-a"));
      rerender(<SessionConfigEditor {...defaultProps} skippedSteps={["step-a"]} disabled={true} />);

      // Then: Save button is rendered (change exists) but must be disabled
      const saveBtn = screen.getByRole("button", { name: /^save$/i });
      expect(saveBtn).toBeDisabled();
    });
  });

  describe("updateSessionSettings invocation", () => {
    it("updateSessionSettings is called with correct args when Save button is clicked", async () => {
      // Given: skip step-a, then uncheck and save
      const steps = [makeStep("step-a"), makeStep("step-b")];
      mockGetDefaults.mockResolvedValue({ steps });
      render(
        <SessionConfigEditor
          {...defaultProps}
          skippedSteps={[]}
        />
      );
      await waitFor(() => screen.getByLabelText("step-a"));

      // When: check step-a
      await userEvent.click(screen.getByLabelText("step-a"));
      const saveBtn = await screen.findByRole("button", { name: /^save$/i });
      await userEvent.click(saveBtn);

      // Then
      await waitFor(() => {
        expect(mockUpdateSettings).toHaveBeenCalledWith("session-1", {
          configPath: undefined,
          skippedSteps: ["step-a"],
        });
      });
    });

    it("onSessionUpdated is called after save", async () => {
      // Given
      const steps = [makeStep("step-a")];
      mockGetDefaults.mockResolvedValue({ steps });
      const updatedSession = makeSession({ id: "session-1", phase: "Planned" });
      mockUpdateSettings.mockResolvedValue(updatedSession);
      const onSessionUpdated = vi.fn();
      render(
        <SessionConfigEditor
          {...defaultProps}
          onSessionUpdated={onSessionUpdated}
        />
      );
      await waitFor(() => screen.getByLabelText("step-a"));

      // When
      await userEvent.click(screen.getByLabelText("step-a"));
      await userEvent.click(await screen.findByRole("button", { name: /^save$/i }));

      // Then
      await waitFor(() => {
        expect(onSessionUpdated).toHaveBeenCalledWith(updatedSession);
      });
    });
  });

  describe("planFailed channel handling", () => {
    it("shows error and calls onError when planFailed event is received", async () => {
      // Given
      mockListConfigs.mockResolvedValue([{ name: "custom.yaml", path: "/path/custom.yaml" }]);
      const onError = vi.fn();
      mockRegenerate.mockImplementation(async (_sessionId, channel) => {
        channel.onmessage({ event: "planFailed", data: { error: "plan generation failed" } });
      });
      render(<SessionConfigEditor {...defaultProps} onError={onError} />);
      await waitFor(() => screen.getByLabelText("Config"));

      // When: select custom config to show "Save & Regenerate Plan" button
      await userEvent.selectOptions(screen.getByLabelText("Config"), "/path/custom.yaml");
      const regenBtn = await screen.findByRole("button", { name: /save & regenerate plan/i });
      await userEvent.click(regenBtn);

      // Then: error message is displayed and onError is called
      await waitFor(() => {
        expect(screen.getByText("plan generation failed")).toBeInTheDocument();
        expect(onError).toHaveBeenCalledWith("plan generation failed");
      });
    });
  });

  describe("Checkbox tri-state (parent node)", () => {
    it("Parent checkbox is checked=true when all child checkboxes are on", async () => {
      // Given: parent node has two children
      const child1 = makeStep("parent/child1");
      const child2 = makeStep("parent/child2");
      const parent = makeStep("parent", [child1, child2]);
      parent.expandedStepIds = ["parent/child1", "parent/child2"];
      mockGetDefaults.mockResolvedValue({ steps: [parent] });
      render(
        <SessionConfigEditor
          {...defaultProps}
          skippedSteps={["parent/child1", "parent/child2"]}
        />
      );

      // When
      await waitFor(() => screen.getByLabelText("parent"));

      // Then: parent checkbox is checked
      const parentCheckbox = screen.getByLabelText("parent") as HTMLInputElement;
      expect(parentCheckbox.checked).toBe(true);
      expect(parentCheckbox.indeterminate).toBe(false);
    });

    it("Parent checkbox is indeterminate when some child checkboxes are on", async () => {
      // Given
      const child1 = makeStep("parent/child1");
      const child2 = makeStep("parent/child2");
      const parent = makeStep("parent", [child1, child2]);
      parent.expandedStepIds = ["parent/child1", "parent/child2"];
      mockGetDefaults.mockResolvedValue({ steps: [parent] });
      render(
        <SessionConfigEditor
          {...defaultProps}
          skippedSteps={["parent/child1"]}
        />
      );

      // When
      await waitFor(() => screen.getByLabelText("parent"));

      // Then: parent checkbox is indeterminate
      const parentCheckbox = screen.getByLabelText("parent") as HTMLInputElement;
      expect(parentCheckbox.indeterminate).toBe(true);
    });
  });
});
