import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  createSession,
  regenerateSessionPlan,
  type EventChannel,
} from "../lib/commands";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
  },
}));

describe("planning command formal-spec payloads", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("sends formalSpec in camelCase for initial plan generation", async () => {
    vi.mocked(invoke).mockResolvedValueOnce({ id: "session-1" });
    vi.mocked(invoke).mockResolvedValueOnce(undefined);

    const params = {
      input: "formal command test",
      baseDir: ".",
      formalSpec: true,
    } as Parameters<typeof createSession>[0];
    const channel = {} as EventChannel;

    await createSession(params, channel);

    expect(invoke).toHaveBeenNthCalledWith(
      2,
      "generate_plan_for_draft",
      expect.objectContaining({
        sessionId: "session-1",
        request: expect.objectContaining({ formalSpec: true }),
        channel,
      }),
    );
  });

  it("keeps formalSpec explicitly disabled for session-plan regeneration", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(undefined);
    vi.mocked(invoke).mockResolvedValueOnce("# existing plan");

    await regenerateSessionPlan("session-1", {} as EventChannel);

    expect(invoke).toHaveBeenNthCalledWith(
      1,
      "regenerate_session_plan",
      expect.objectContaining({
        sessionId: "session-1",
        request: expect.objectContaining({ formalSpec: false }),
      }),
    );
  });
});
