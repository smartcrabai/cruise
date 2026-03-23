import { afterEach, vi } from "vitest";
import { act, cleanup, configure } from "@testing-library/react";

afterEach(cleanup);

(globalThis as unknown as Record<string, unknown>).jest = {
  advanceTimersByTime: (ms: number) => vi.advanceTimersByTime(ms),
};

configure({
  unstable_advanceTimersWrapper: async (fn) => {
    await act(async () => {
      fn();
    });
  },
});
