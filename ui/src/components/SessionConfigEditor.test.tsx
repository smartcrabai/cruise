import { render, screen, cleanup, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { BUILTIN_CONFIG_PATH, type Session, type SkippableStepDto } from "../types";

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
  getSessionDag: vi.fn(),
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
  onBusyChange: vi.fn(),
  onError: vi.fn(),
  disabled: false,
};

beforeEach(() => {
  vi.clearAllMocks();
  mockListConfigs.mockResolvedValue([]);
  mockGetDefaults.mockResolvedValue({ steps: [], afterPrSteps: [], defaultSkippedSteps: [] });
  mockUpdateSettings.mockResolvedValue(makeSession());
  mockRegenerate.mockResolvedValue("");
});

afterEach(() => cleanup());

describe("SessionConfigEditor", () => {
  describe("Built-in default pin (configPath = __builtin__)", () => {
    it("does not synthesize a duplicate config entry for the sentinel", async () => {
      // Given: the session is pinned to the built-in default and listConfigs
      // returns only real files (the backend never emits the sentinel)
      mockListConfigs.mockResolvedValue([
        { name: "cruise.yaml", path: "/home/user/project/cruise.yaml" },
      ]);
      render(<SessionConfigEditor {...defaultProps} configPath={BUILTIN_CONFIG_PATH} />);

      // Then: exactly one option carries the sentinel value — the static
      // "Built-in default" option, never a synthetic per-file entry; and it is selected
      const select = await screen.findByLabelText("Config");
      await waitFor(() => {
        expect(select).toHaveValue(BUILTIN_CONFIG_PATH);
      });
      expect(select.querySelectorAll(`option[value="${BUILTIN_CONFIG_PATH}"]`)).toHaveLength(1);
    });
  });

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
      mockGetDefaults.mockResolvedValue({ steps, afterPrSteps: [], defaultSkippedSteps: [] });
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

    it("'Save' button is also shown alongside 'Save & Regenerate Plan' when config is changed", async () => {
      // Given
      mockListConfigs.mockResolvedValue([{ name: "custom.yaml", path: "/path/custom.yaml" }]);
      render(<SessionConfigEditor {...defaultProps} />);
      await waitFor(() => screen.getByLabelText("Config"));

      // When: change config
      await userEvent.selectOptions(screen.getByLabelText("Config"), "/path/custom.yaml");

      // Then: both buttons are shown so the user can save without regenerating the plan
      await waitFor(() => {
        expect(screen.getByRole("button", { name: "Save" })).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "Save & Regenerate Plan" })).toBeInTheDocument();
      });
    });

    it("clicking 'Save' after a config change calls updateSessionSettings but not regenerateSessionPlan", async () => {
      // Given
      mockListConfigs.mockResolvedValue([{ name: "custom.yaml", path: "/path/custom.yaml" }]);
      render(<SessionConfigEditor {...defaultProps} />);
      await waitFor(() => screen.getByLabelText("Config"));

      // When: change config and click plain "Save"
      await userEvent.selectOptions(screen.getByLabelText("Config"), "/path/custom.yaml");
      const saveBtn = await screen.findByRole("button", { name: "Save" });
      await userEvent.click(saveBtn);

      // Then: settings are saved but the plan is not regenerated
      await waitFor(() => {
        expect(mockUpdateSettings).toHaveBeenCalledWith("session-1", {
          configPath: "/path/custom.yaml",
          skippedSteps: [],
        });
      });
      expect(mockRegenerate).not.toHaveBeenCalled();
    });

    it("clicking 'Save & Regenerate Plan' after a config change calls both updateSessionSettings and regenerateSessionPlan", async () => {
      // Given
      mockListConfigs.mockResolvedValue([{ name: "custom.yaml", path: "/path/custom.yaml" }]);
      render(<SessionConfigEditor {...defaultProps} />);
      await waitFor(() => screen.getByLabelText("Config"));

      // When: change config and click "Save & Regenerate Plan"
      await userEvent.selectOptions(screen.getByLabelText("Config"), "/path/custom.yaml");
      const regenBtn = await screen.findByRole("button", { name: "Save & Regenerate Plan" });
      await userEvent.click(regenBtn);

      // Then: both settings are saved and the plan is regenerated
      await waitFor(() => {
        expect(mockUpdateSettings).toHaveBeenCalledWith("session-1", {
          configPath: "/path/custom.yaml",
          skippedSteps: [],
        });
        expect(mockRegenerate).toHaveBeenCalledWith("session-1", expect.anything());
      });
    });
  });

  describe("Save button disabled state", () => {
    it("Save button is disabled when disabled=true", async () => {
      // Given: re-render with disabled=true after changing skip steps,
      // to simulate state where there are changes but disabled=true
      const steps = [makeStep("step-a")];
      mockGetDefaults.mockResolvedValue({ steps, afterPrSteps: [], defaultSkippedSteps: [] });
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

    it("both 'Save' and 'Save & Regenerate Plan' are disabled while plain Save is in flight", async () => {
      // Given: config changed, updateSessionSettings never resolves
      mockListConfigs.mockResolvedValue([{ name: "custom.yaml", path: "/path/custom.yaml" }]);
      mockUpdateSettings.mockImplementation(() => new Promise(() => {}));
      render(<SessionConfigEditor {...defaultProps} />);
      await waitFor(() => screen.getByLabelText("Config"));
      await userEvent.selectOptions(screen.getByLabelText("Config"), "/path/custom.yaml");

      // When: click plain "Save"
      const saveBtn = await screen.findByRole("button", { name: "Save" });
      await userEvent.click(saveBtn);

      // Then: both buttons become disabled so nothing else can run concurrently
      await waitFor(() => {
        expect(screen.getByRole("button", { name: "Saving..." })).toBeDisabled();
        expect(screen.getByRole("button", { name: "Save & Regenerate Plan" })).toBeDisabled();
      });
    });
  });

  describe("Save failure handling", () => {
    it("shows an error, calls onError, and re-enables 'Save' when updateSessionSettings rejects", async () => {
      // Given: skip step changed, updateSessionSettings rejects
      const steps = [makeStep("step-a")];
      mockGetDefaults.mockResolvedValue({ steps, afterPrSteps: [], defaultSkippedSteps: [] });
      mockUpdateSettings.mockRejectedValue(new Error("save failed"));
      const onError = vi.fn();
      render(<SessionConfigEditor {...defaultProps} onError={onError} />);
      await waitFor(() => screen.getByLabelText("step-a"));

      // When: check step-a and click "Save"
      await userEvent.click(screen.getByLabelText("step-a"));
      const saveBtn = await screen.findByRole("button", { name: /^save$/i });
      await userEvent.click(saveBtn);

      // Then: the error is displayed and onError is called
      await waitFor(() => {
        expect(screen.getByText(/save failed/)).toBeInTheDocument();
        expect(onError).toHaveBeenCalledWith(expect.stringContaining("save failed"));
      });

      // And: isSaving resets so the button label reverts to "Save" and is re-enabled
      const resetSaveBtn = screen.getByRole("button", { name: "Save" });
      expect(resetSaveBtn).not.toBeDisabled();
    });
  });

  describe("Save-without-regenerate note", () => {
    it("is shown only when the config has changed", async () => {
      // Given
      const steps = [makeStep("step-a")];
      mockGetDefaults.mockResolvedValue({ steps, afterPrSteps: [], defaultSkippedSteps: [] });
      mockListConfigs.mockResolvedValue([{ name: "custom.yaml", path: "/path/custom.yaml" }]);
      render(<SessionConfigEditor {...defaultProps} />);
      await waitFor(() => screen.getByLabelText("Config"));

      // Then: no changes yet, so the note is absent
      expect(
        screen.queryByText(/Saving without regenerating keeps the existing plan/),
      ).not.toBeInTheDocument();

      // When: skip-only change
      await userEvent.click(screen.getByLabelText("step-a"));

      // Then: skip-only change does not show the note either
      await waitFor(() => screen.getByRole("button", { name: /^save$/i }));
      expect(
        screen.queryByText(/Saving without regenerating keeps the existing plan/),
      ).not.toBeInTheDocument();

      // When: config is also changed
      await userEvent.selectOptions(screen.getByLabelText("Config"), "/path/custom.yaml");

      // Then: the note appears
      await waitFor(() => {
        expect(
          screen.getByText(/Saving without regenerating keeps the existing plan/),
        ).toBeInTheDocument();
      });
    });
  });

  describe("updateSessionSettings invocation", () => {
    it("updateSessionSettings is called with correct args when Save button is clicked", async () => {
      // Given: skip step-a, then uncheck and save
      const steps = [makeStep("step-a"), makeStep("step-b")];
      mockGetDefaults.mockResolvedValue({ steps, afterPrSteps: [], defaultSkippedSteps: [] });
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
          configPath: "",
          skippedSteps: ["step-a"],
        });
      });
    });

    it("onSessionUpdated is called after save", async () => {
      // Given
      const steps = [makeStep("step-a")];
      mockGetDefaults.mockResolvedValue({ steps, afterPrSteps: [], defaultSkippedSteps: [] });
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
        channel.onmessage({ event: "planFailed", data: { sessionId: "session-1", error: "plan generation failed" } });
        return "";
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
      mockGetDefaults.mockResolvedValue({ steps: [parent], afterPrSteps: [], defaultSkippedSteps: [] });
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
      mockGetDefaults.mockResolvedValue({ steps: [parent], afterPrSteps: [], defaultSkippedSteps: [] });
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

  describe("Failed/Suspended phase — config select disabled", () => {
    it("Config select is disabled when phase is 'Failed'", async () => {
      // Given: component rendered with Failed phase
      mockListConfigs.mockResolvedValue([{ name: "custom.yaml", path: "/path/custom.yaml" }]);
      render(<SessionConfigEditor {...defaultProps} phase="Failed" />);
      await waitFor(() => screen.getByLabelText("Config"));

      // Then: config select is disabled (config swap not allowed for Failed sessions)
      const configSelect = screen.getByLabelText("Config") as HTMLSelectElement;
      expect(configSelect).toBeDisabled();
    });

    it("Config select is disabled when phase is 'Suspended'", async () => {
      // Given: component rendered with Suspended phase
      mockListConfigs.mockResolvedValue([{ name: "custom.yaml", path: "/path/custom.yaml" }]);
      render(<SessionConfigEditor {...defaultProps} phase="Suspended" />);
      await waitFor(() => screen.getByLabelText("Config"));

      // Then: config select is disabled
      const configSelect = screen.getByLabelText("Config") as HTMLSelectElement;
      expect(configSelect).toBeDisabled();
    });

    it("Config select is enabled when phase is 'Planned'", async () => {
      // Given: component rendered with Planned phase (default)
      mockListConfigs.mockResolvedValue([{ name: "custom.yaml", path: "/path/custom.yaml" }]);
      render(<SessionConfigEditor {...defaultProps} phase="Planned" />);
      await waitFor(() => screen.getByLabelText("Config"));

      // Then: config select is enabled for Planned
      const configSelect = screen.getByLabelText("Config") as HTMLSelectElement;
      expect(configSelect).not.toBeDisabled();
    });
  });

  describe("Failed/Suspended phase — Current Step selector", () => {
    it("Current Step selector is shown when phase is 'Failed'", async () => {
      // Given: a Failed session with steps available
      const steps = [makeStep("step-a"), makeStep("step-b")];
      mockGetDefaults.mockResolvedValue({ steps, afterPrSteps: [], defaultSkippedSteps: [] });
      render(<SessionConfigEditor {...defaultProps} phase="Failed" currentStep="step-a" />);

      // Then: a "Current Step" label/select is visible
      await waitFor(() => {
        expect(screen.getByLabelText(/current step/i)).toBeInTheDocument();
      });
    });

    it("Current Step selector is shown when phase is 'Suspended'", async () => {
      // Given: a Suspended session with steps available
      const steps = [makeStep("step-a"), makeStep("step-b")];
      mockGetDefaults.mockResolvedValue({ steps, afterPrSteps: [], defaultSkippedSteps: [] });
      render(<SessionConfigEditor {...defaultProps} phase="Suspended" currentStep="step-a" />);

      // Then: a "Current Step" label/select is visible
      await waitFor(() => {
        expect(screen.getByLabelText(/current step/i)).toBeInTheDocument();
      });
    });

    it("Current Step selector is NOT shown for 'Planned' phase", async () => {
      // Given: a Planned session (no in-progress step to resume from)
      const steps = [makeStep("step-a"), makeStep("step-b")];
      mockGetDefaults.mockResolvedValue({ steps, afterPrSteps: [], defaultSkippedSteps: [] });
      render(<SessionConfigEditor {...defaultProps} phase="Planned" />);
      await waitFor(() => screen.getByLabelText("step-a"));

      // Then: Current Step selector is absent
      expect(screen.queryByLabelText(/current step/i)).not.toBeInTheDocument();
    });

    it("Save button appears when Current Step selection changes", async () => {
      // Given: Failed session with step-a as current step and steps available
      const steps = [makeStep("step-a"), makeStep("step-b")];
      mockGetDefaults.mockResolvedValue({ steps, afterPrSteps: [], defaultSkippedSteps: [] });
      render(
        <SessionConfigEditor
          {...defaultProps}
          phase="Failed"
          currentStep="step-a"
        />
      );
      await waitFor(() => screen.getByLabelText(/current step/i));

      // When: change current step to step-b
      await userEvent.selectOptions(screen.getByLabelText(/current step/i), "step-b");

      // Then: Save button appears
      await waitFor(() => {
        expect(screen.getByRole("button", { name: /^save$/i })).toBeInTheDocument();
      });
    });

    it("updateSessionSettings is called with currentStep when saved", async () => {
      // Given: Failed session, current step is step-a, steps list available
      const steps = [makeStep("step-a"), makeStep("step-b")];
      mockGetDefaults.mockResolvedValue({ steps, afterPrSteps: [], defaultSkippedSteps: [] });
      render(
        <SessionConfigEditor
          {...defaultProps}
          phase="Failed"
          currentStep="step-a"
        />
      );
      await waitFor(() => screen.getByLabelText(/current step/i));

      // When: select step-b and click Save
      await userEvent.selectOptions(screen.getByLabelText(/current step/i), "step-b");
      const saveBtn = await screen.findByRole("button", { name: /^save$/i });
      await userEvent.click(saveBtn);

      // Then: updateSessionSettings is called with the new currentStep
      await waitFor(() => {
        expect(mockUpdateSettings).toHaveBeenCalledWith(
          "session-1",
          expect.objectContaining({ currentStep: "step-b" })
        );
      });
    });

    it("updateSessionSettings is called with currentStep=null when '(from beginning)' is selected", async () => {
      // Given: Failed session, current step is step-a
      const steps = [makeStep("step-a"), makeStep("step-b")];
      mockGetDefaults.mockResolvedValue({ steps, afterPrSteps: [], defaultSkippedSteps: [] });
      render(
        <SessionConfigEditor
          {...defaultProps}
          phase="Failed"
          currentStep="step-a"
        />
      );
      await waitFor(() => screen.getByLabelText(/current step/i));

      // When: select "(from beginning)" (empty value → null)
      await userEvent.selectOptions(screen.getByLabelText(/current step/i), "");
      const saveBtn = await screen.findByRole("button", { name: /^save$/i });
      await userEvent.click(saveBtn);

      // Then: currentStep is null (clear — run from beginning)
      await waitFor(() => {
        expect(mockUpdateSettings).toHaveBeenCalledWith(
          "session-1",
          expect.objectContaining({ currentStep: null })
        );
      });
    });
  });

  describe("listConfigs invocation", () => {
    it("calls listConfigs with { baseDir } for a non-repo session", async () => {
      // Given: a session with a baseDir and no repo
      render(<SessionConfigEditor {...defaultProps} baseDir="/home/user/project" />);

      // Then: listConfigs is called with the session's baseDir
      await waitFor(() => {
        expect(mockListConfigs).toHaveBeenCalledWith({ baseDir: "/home/user/project" });
      });
    });

    it("calls listConfigs with { repo } for a repo-backed session, ignoring baseDir", async () => {
      // Given: a session backed by a repo clone
      render(<SessionConfigEditor {...defaultProps} baseDir="/tmp/clones/abc" repo="owner/repo" />);

      // Then: listConfigs is called with repo instead of baseDir (clone dir is transient)
      await waitFor(() => {
        expect(mockListConfigs).toHaveBeenCalledWith({ repo: "owner/repo" });
      });
    });
  });

  describe("Draft phase — config edits save without regenerating the plan", () => {
    it("Config select is enabled when phase is 'Draft'", async () => {
      // Given: component rendered with Draft phase
      mockListConfigs.mockResolvedValue([{ name: "custom.yaml", path: "/path/custom.yaml", source: "local" }]);
      render(<SessionConfigEditor {...defaultProps} phase="Draft" />);
      await waitFor(() => screen.getByLabelText("Config"));

      // Then: config select is enabled (Draft allows changing the config)
      const configSelect = screen.getByLabelText("Config") as HTMLSelectElement;
      expect(configSelect).not.toBeDisabled();
    });

    it("shows a plain 'Save' button (not 'Save & Regenerate Plan') when config changes in Draft phase", async () => {
      // Given: Draft-phase session with a selectable config
      mockListConfigs.mockResolvedValue([{ name: "custom.yaml", path: "/path/custom.yaml", source: "local" }]);
      render(<SessionConfigEditor {...defaultProps} phase="Draft" />);
      await waitFor(() => screen.getByLabelText("Config"));

      // When: changing the config
      await userEvent.selectOptions(screen.getByLabelText("Config"), "/path/custom.yaml");

      // Then: the plain Save button appears, and the regenerate variant does not
      await waitFor(() => {
        expect(screen.getByRole("button", { name: /^save$/i })).toBeInTheDocument();
      });
      expect(screen.queryByRole("button", { name: /save & regenerate plan/i })).not.toBeInTheDocument();
    });

    it("calls updateSessionSettings but not regenerateSessionPlan when saving a config change in Draft phase", async () => {
      // Given: Draft-phase session with a selectable config
      mockListConfigs.mockResolvedValue([{ name: "custom.yaml", path: "/path/custom.yaml", source: "local" }]);
      render(<SessionConfigEditor {...defaultProps} phase="Draft" />);
      await waitFor(() => screen.getByLabelText("Config"));

      // When: changing the config and clicking Save
      await userEvent.selectOptions(screen.getByLabelText("Config"), "/path/custom.yaml");
      const saveBtn = await screen.findByRole("button", { name: /^save$/i });
      await userEvent.click(saveBtn);

      // Then: settings are persisted via the plain save path, and no plan regeneration is triggered
      await waitFor(() => {
        expect(mockUpdateSettings).toHaveBeenCalledWith(
          "session-1",
          expect.objectContaining({ configPath: "/path/custom.yaml" })
        );
      });
      expect(mockRegenerate).not.toHaveBeenCalled();
    });
  });
});
