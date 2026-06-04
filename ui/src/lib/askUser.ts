/** Detail payload for the {@link ASK_USER_EVENT} window event. */
export interface AskUserDetail {
  sessionId: string;
  question: string;
}

/**
 * Window event name that a PlanEvent `askUserRequired` is forwarded as.
 *
 * Plan event handlers dispatch this so the single top-level `AskUserDialog`
 * (mounted in `App`) can prompt the user regardless of which flow triggered it.
 */
export const ASK_USER_EVENT = "cruise:ask-user";
