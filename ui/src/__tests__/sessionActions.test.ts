import { describe, it, expect } from "vitest";
import { getSessionActions } from "../lib/sessionActions";
import type { Session } from "../types";

function makeSession(overrides: Partial<Session> = {}): Session {
  return {
    id: "session-1",
    phase: "Planned",
    configSource: "default.yaml",
    baseDir: "/home/user/project",
    input: "test task",
    createdAt: "2026-01-01T00:00:00Z",
    workspaceMode: "Worktree",
    ...overrides,
  };
}

describe("getSessionActions", () => {
  // --- Running phase ---------------------------------------------------------

  describe("Running phase", () => {
    it("shows Resume instead of workspace selection even when currentStep is null", () => {
      // Given: a Running session with no currentStep (as happens in GUI-started runs)
      const session = makeSession({ phase: "Running", currentStep: undefined });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: workspace selection buttons are absent; resume is shown
      expect(actions.showCreateWorktree).toBe(false);
      expect(actions.showRun).toBe(true);
      expect(actions.runLabel).toBe("Resume");
    });

    it("shows Cancel when the session is being run locally", () => {
      // Given: Running session with local execution in progress
      const session = makeSession({ phase: "Running" });

      // When
      const actions = getSessionActions(session, "running");

      // Then: Cancel is shown
      expect(actions.showCancel).toBe(true);
    });

    it("hides Cancel when status is idle", () => {
      // Given: Running phase but no local execution in progress
      const session = makeSession({ phase: "Running" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: Cancel is absent
      expect(actions.showCancel).toBe(false);
    });

    it("hides Delete while phase is Running", () => {
      // Given: Running session
      const session = makeSession({ phase: "Running" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: cannot delete a running session
      expect(actions.showDelete).toBe(false);
    });
  });

  // --- Awaiting Approval phase -----------------------------------------------

  describe("Awaiting Approval phase", () => {
    it("shows Approve when planAvailable is true", () => {
      // Given: session awaiting approval with a valid plan
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: Approve button is visible
      expect(actions.showApprove).toBe(true);
    });

    it("hides Approve when planAvailable is false", () => {
      // Given: session awaiting approval but plan is absent/empty
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: false });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: Approve button is absent
      expect(actions.showApprove).toBe(false);
    });

    it("hides Approve when planAvailable is undefined (safe default)", () => {
      // Given: session awaiting approval with no planAvailable field (e.g. legacy DTO)
      const session = makeSession({ phase: "Awaiting Approval" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: Approve button is absent (treat undefined as false)
      expect(actions.showApprove).toBe(false);
    });

    it("hides workspace selection buttons", () => {
      // Given: Awaiting Approval session
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: fresh-run workspace buttons are absent (not yet Planned)
      expect(actions.showCreateWorktree).toBe(false);
    });

    it("hides the run button", () => {
      // Given: Awaiting Approval session
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: can't run until approved
      expect(actions.showRun).toBe(false);
    });

    it("shows Delete", () => {
      // Given: Awaiting Approval session
      const session = makeSession({ phase: "Awaiting Approval" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: can discard the session
      expect(actions.showDelete).toBe(true);
    });
  });

  // --- Planned phase ---------------------------------------------------------

  describe("Planned phase", () => {
    it("shows workspace selection buttons for a fresh run", () => {
      // Given: Planned session (fresh, ready to run)
      const session = makeSession({ phase: "Planned" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: user must choose a workspace before running
      expect(actions.showCreateWorktree).toBe(true);
    });

    it("shows Replan button", () => {
      // Given: Planned session
      const session = makeSession({ phase: "Planned" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then
      expect(actions.showReplan).toBe(true);
    });

    it("hides Approve button (already approved)", () => {
      // Given: Planned session
      const session = makeSession({ phase: "Planned" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then
      expect(actions.showApprove).toBe(false);
    });

    it("hides the Resume/Retry run button (workspace selection is used instead)", () => {
      // Given: Planned session
      const session = makeSession({ phase: "Planned" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: workspace selection replaces a plain run button
      expect(actions.showRun).toBe(false);
    });

    it("shows Delete", () => {
      // Given: Planned session
      const session = makeSession({ phase: "Planned" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then
      expect(actions.showDelete).toBe(true);
    });
  });

  // --- Suspended phase -------------------------------------------------------

  describe("Suspended phase", () => {
    it("shows Resume with label 'Resume'", () => {
      // Given: Suspended session (interrupted, can be resumed)
      const session = makeSession({ phase: "Suspended" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then
      expect(actions.showRun).toBe(true);
      expect(actions.runLabel).toBe("Resume");
    });

    it("hides workspace selection buttons (resume, not fresh run)", () => {
      // Given: Suspended session
      const session = makeSession({ phase: "Suspended" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then
      expect(actions.showCreateWorktree).toBe(false);
    });

    it("shows Reset to Planned", () => {
      // Given: Suspended session
      const session = makeSession({ phase: "Suspended" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then
      expect(actions.showReset).toBe(true);
    });
  });

  // --- Failed phase ----------------------------------------------------------

  describe("Failed phase", () => {
    it("shows Retry with label 'Retry'", () => {
      // Given: Failed session
      const session = makeSession({ phase: "Failed", phaseError: "something went wrong" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then
      expect(actions.showRun).toBe(true);
      expect(actions.runLabel).toBe("Retry");
    });

    it("shows Reset to Planned", () => {
      // Given: Failed session
      const session = makeSession({ phase: "Failed" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then
      expect(actions.showReset).toBe(true);
    });

    it("hides workspace selection buttons (retry, not fresh run)", () => {
      // Given: Failed session
      const session = makeSession({ phase: "Failed" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then
      expect(actions.showCreateWorktree).toBe(false);
    });
  });

  // --- Completed phase -------------------------------------------------------

  describe("Completed phase", () => {
    it("shows Reset to Planned", () => {
      // Given: Completed session
      const session = makeSession({ phase: "Completed" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then
      expect(actions.showReset).toBe(true);
    });

    it("hides workspace selection and run buttons", () => {
      // Given: Completed session
      const session = makeSession({ phase: "Completed" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: no further execution options
      expect(actions.showCreateWorktree).toBe(false);
      expect(actions.showRun).toBe(false);
    });

    it("shows Delete", () => {
      // Given: Completed session
      const session = makeSession({ phase: "Completed" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then
      expect(actions.showDelete).toBe(true);
    });
  });

  // --- Delete button ---------------------------------------------------------

  describe("Delete button", () => {
    it("shows Delete for all non-Running phases", () => {
      // Given: each phase except Running
      const phases: Array<Session["phase"]> = [
        "Awaiting Approval",
        "Planned",
        "Suspended",
        "Failed",
        "Completed",
      ];

      for (const phase of phases) {
        // When / Then
        const actions = getSessionActions(makeSession({ phase }), "idle");
        expect(actions.showDelete, `expected showDelete for phase ${phase}`).toBe(true);
      }
    });

    it("hides Delete when phase is Running", () => {
      // Given: Running session
      const session = makeSession({ phase: "Running" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: cannot delete a running session; must cancel first
      expect(actions.showDelete).toBe(false);
    });
  });

  // --- status === "running" ---------------------------------------------------

  describe("when status is 'running'", () => {
    it("hides all action buttons except Cancel", () => {
      // Given: a Planned session with an active local run
      const session = makeSession({ phase: "Planned" });

      // When
      const actions = getSessionActions(session, "running");

      // Then: only Cancel is shown
      expect(actions.showCancel).toBe(true);
      expect(actions.showApprove).toBe(false);
      expect(actions.showCreateWorktree).toBe(false);
      expect(actions.showRun).toBe(false);
      expect(actions.showReset).toBe(false);
      expect(actions.showReplan).toBe(false);
      expect(actions.showDelete).toBe(false);
    });
  });

  // --- Cancel button ---------------------------------------------------------

  describe("Cancel button", () => {
    it("shows Cancel only when status is 'running'", () => {
      // Given: any phase with local execution in progress
      const session = makeSession({ phase: "Running" });

      // When
      const actions = getSessionActions(session, "running");

      // Then
      expect(actions.showCancel).toBe(true);
    });

    it("hides Cancel when status is 'idle' regardless of phase", () => {
      // Given: Running session but no local execution
      const session = makeSession({ phase: "Running" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then
      expect(actions.showCancel).toBe(false);
    });
  });
});
