import { describe, it, expect } from "vitest";
import { getSessionActions, isApprovalReady } from "../lib/sessionActions";
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

    it("shows Fix when planAvailable is true", () => {
      // Given: session awaiting approval with a valid plan
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: Fix button is visible (plan review loop allows fix)
      expect(actions.showFix).toBe(true);
    });

    it("hides Fix when planAvailable is false", () => {
      // Given: session awaiting approval but no plan yet
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: false });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: nothing to fix if there is no plan
      expect(actions.showFix).toBe(false);
    });

    it("hides Fix when planAvailable is undefined (safe default)", () => {
      // Given: session awaiting approval with no planAvailable field
      const session = makeSession({ phase: "Awaiting Approval" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: treat undefined as false
      expect(actions.showFix).toBe(false);
    });

    it("shows Ask when planAvailable is true", () => {
      // Given: session awaiting approval with a valid plan
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: Ask button is visible (user can ask about the plan before approving)
      expect(actions.showAsk).toBe(true);
    });

    it("hides Ask when planAvailable is false", () => {
      // Given: session awaiting approval but no plan yet
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: false });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: nothing to ask about if there is no plan
      expect(actions.showAsk).toBe(false);
    });

    it("hides Ask when planAvailable is undefined (safe default)", () => {
      // Given: session awaiting approval with no planAvailable field
      const session = makeSession({ phase: "Awaiting Approval" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: treat undefined as false
      expect(actions.showAsk).toBe(false);
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

    it("hides Fix button (Planned uses Replan instead of Fix)", () => {
      // Given: Planned session
      const session = makeSession({ phase: "Planned" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: Fix is an Awaiting Approval action; Planned uses showReplan
      expect(actions.showFix).toBe(false);
    });

    it("hides Ask button (Ask is only available during plan review)", () => {
      // Given: Planned session
      const session = makeSession({ phase: "Planned" });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: Ask is only for Awaiting Approval review, not Planned
      expect(actions.showAsk).toBe(false);
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
      expect(actions.showFix).toBe(false);
      expect(actions.showAsk).toBe(false);
      expect(actions.showCreateWorktree).toBe(false);
      expect(actions.showRun).toBe(false);
      expect(actions.showReset).toBe(false);
      expect(actions.showReplan).toBe(false);
      expect(actions.showDelete).toBe(false);
    });

    it("hides Fix and Ask even when Awaiting Approval session has planAvailable", () => {
      // Given: an Awaiting Approval session with a plan, but execution is in progress
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });

      // When
      const actions = getSessionActions(session, "running");

      // Then: Fix/Ask are suppressed while running (only Cancel is active)
      expect(actions.showFix).toBe(false);
      expect(actions.showAsk).toBe(false);
      expect(actions.showCancel).toBe(true);
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

  // --- post-run transitioning states ----------------------------------------

  describe("post-run transitioning states (status completed/failed/cancelled with stale Running phase)", () => {
    it.each(["completed", "failed", "cancelled"] as const)(
      "hides Resume and Reset when status is '%s' and phase is still 'Running'",
      (terminalStatus) => {
        const session = makeSession({ phase: "Running" });
        const actions = getSessionActions(session, terminalStatus);

        expect(actions.showRun).toBe(false);
        expect(actions.showReset).toBe(false);
      },
    );

    it("hides Cancel when status is 'completed'", () => {
      const session = makeSession({ phase: "Running" });
      const actions = getSessionActions(session, "completed");

      expect(actions.showCancel).toBe(false);
    });

    it("hides Delete when phase is still 'Running' regardless of status", () => {
      const session = makeSession({ phase: "Running" });
      const actions = getSessionActions(session, "completed");

      expect(actions.showDelete).toBe(false);
    });
  });

  // --- isFixing flag ---------------------------------------------------------

  describe("when isFixing is true", () => {
    it("hides Approve, Fix, and Ask during an active plan fix", () => {
      // Given: an Awaiting Approval session with a plan, and a fix is currently running
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });

      // When
      const actions = getSessionActions(session, "idle", true);

      // Then: all review-stage actions are suppressed while fixing is in progress
      expect(actions.showApprove).toBe(false);
      expect(actions.showFix).toBe(false);
      expect(actions.showAsk).toBe(false);
    });

    it("keeps Delete visible during an active plan fix", () => {
      // Given: an Awaiting Approval session with a plan, and a fix is currently running
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });

      // When
      const actions = getSessionActions(session, "idle", true);

      // Then: delete is unaffected by the fixing state
      expect(actions.showDelete).toBe(true);
    });

    it("isFixing=false produces the same result as omitting the argument", () => {
      // Given: an Awaiting Approval session with a plan
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });

      // When
      const withFalse = getSessionActions(session, "idle", false);
      const withDefault = getSessionActions(session, "idle");

      // Then: explicitly passing false is the same as the default (not fixing)
      expect(withFalse.showApprove).toBe(withDefault.showApprove);
      expect(withFalse.showFix).toBe(withDefault.showFix);
      expect(withFalse.showAsk).toBe(withDefault.showAsk);
    });
  });

  // --- fixInProgress DTO field (persisted fix-in-progress state) -----------

  describe("when session.fixInProgress is true", () => {
    it("hides Approve, Fix, and Ask via the DTO field without needing the isFixing argument", () => {
      // Given: an Awaiting Approval session whose DTO carries fixInProgress: true
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: true, fixInProgress: true });

      // When: no isFixing argument is passed (the DTO field is the only source)
      const actions = getSessionActions(session, "idle");

      // Then: all review-stage actions are suppressed by the persisted field alone
      expect(actions.showApprove).toBe(false);
      expect(actions.showFix).toBe(false);
      expect(actions.showAsk).toBe(false);
    });

    it("keeps Delete visible when fixInProgress is true", () => {
      // Given: Awaiting Approval session with fixInProgress: true
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: true, fixInProgress: true });

      // When
      const actions = getSessionActions(session, "idle");

      // Then: delete is unaffected by the persisted fixing state
      expect(actions.showDelete).toBe(true);
    });

    it("fixInProgress: false produces the same result as omitting the field", () => {
      // Given: two otherwise identical sessions – one with explicit false, one with undefined
      const withFalse = makeSession({ phase: "Awaiting Approval", planAvailable: true, fixInProgress: false });
      const withUndefined = makeSession({ phase: "Awaiting Approval", planAvailable: true });

      // When
      const actionsWithFalse = getSessionActions(withFalse, "idle");
      const actionsWithUndefined = getSessionActions(withUndefined, "idle");

      // Then: both show the full review actions (approval-ready state)
      expect(actionsWithFalse.showApprove).toBe(actionsWithUndefined.showApprove);
      expect(actionsWithFalse.showFix).toBe(actionsWithUndefined.showFix);
      expect(actionsWithFalse.showAsk).toBe(actionsWithUndefined.showAsk);
    });
  });

  // --- isApprovalReady function ---------------------------------------------

  describe("isApprovalReady", () => {
    it("returns true for Awaiting Approval with planAvailable: true", () => {
      // Given
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: true });

      // When / Then
      expect(isApprovalReady(session)).toBe(true);
    });

    it("returns false when fixInProgress is true even when planAvailable is true", () => {
      // Given: the backend has persisted a fix-in-progress flag
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: true, fixInProgress: true });

      // When
      const ready = isApprovalReady(session);

      // Then: the persisted flag overrides the plan-available check
      expect(ready).toBe(false);
    });

    it("returns true when fixInProgress is false (no effect)", () => {
      // Given: fixInProgress is explicitly false
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: true, fixInProgress: false });

      // When / Then: false has no suppressing effect
      expect(isApprovalReady(session)).toBe(true);
    });

    it("returns false when planAvailable is false regardless of fixInProgress", () => {
      // Given: no plan yet
      const session = makeSession({ phase: "Awaiting Approval", planAvailable: false, fixInProgress: false });

      // When / Then
      expect(isApprovalReady(session)).toBe(false);
    });
  });

  // --- post-run states after refresh (phase updated) ------------------------

  describe("post-run states after phase refresh", () => {
    it("shows Reset after refresh: status 'completed', phase 'Completed'", () => {
      const session = makeSession({ phase: "Completed" });
      const actions = getSessionActions(session, "completed");

      expect(actions.showReset).toBe(true);
    });

    it("shows Retry after refresh: status 'failed', phase 'Failed'", () => {
      const session = makeSession({ phase: "Failed" });
      const actions = getSessionActions(session, "failed");

      expect(actions.showRun).toBe(true);
      expect(actions.runLabel).toBe("Retry");
    });

    it("shows Resume after refresh: status 'cancelled', phase 'Suspended'", () => {
      const session = makeSession({ phase: "Suspended" });
      const actions = getSessionActions(session, "cancelled");

      expect(actions.showRun).toBe(true);
      expect(actions.runLabel).toBe("Resume");
    });

    it.each([
      ["completed", "Completed"],
      ["failed", "Failed"],
      ["cancelled", "Suspended"],
    ] as const)("shows Delete after refresh: status '%s', phase '%s'", (terminalStatus, phase) => {
      const session = makeSession({ phase });
      const actions = getSessionActions(session, terminalStatus);

      expect(actions.showDelete).toBe(true);
    });
  });
});
