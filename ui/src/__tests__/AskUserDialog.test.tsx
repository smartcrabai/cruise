import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AskUserDialog } from "../components/AskUserDialog";
import { ASK_USER_EVENT, type AskUserDetail } from "../lib/askUser";

// AskUserDialog calls respondToAsk (a Tauri IPC wrapper) on submit; mock it.
// Variadic so calling with (sessionId, answer) type-checks, and it returns a
// Promise so the component's `.catch(...)` has something to chain.
const { respondToAsk } = vi.hoisted(() => ({
  respondToAsk: vi.fn((..._args: unknown[]) => Promise.resolve()),
}));
vi.mock("../lib/commands", () => ({ respondToAsk }));

/** Dispatch the window event that surfaces an ask_user question. */
function fireAsk(detail: AskUserDetail) {
  act(() => {
    window.dispatchEvent(new CustomEvent(ASK_USER_EVENT, { detail }));
  });
}

describe("AskUserDialog", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders nothing until an ask_user event arrives", () => {
    render(<AskUserDialog />);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("shows the question when an ask_user event is dispatched", () => {
    render(<AskUserDialog />);
    fireAsk({ sessionId: "sess-1", question: "JWT or sessions?" });

    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByText("JWT or sessions?")).toBeInTheDocument();
    // The answer field is accessible by name (not just a placeholder).
    expect(screen.getByRole("textbox", { name: "Your answer" })).toBeInTheDocument();
  });

  it("delivers the typed answer to the correct session and dismisses", async () => {
    render(<AskUserDialog />);
    fireAsk({ sessionId: "sess-42", question: "Which DB?" });

    await userEvent.type(screen.getByRole("textbox", { name: "Your answer" }), "Postgres");
    await userEvent.click(screen.getByRole("button", { name: "Send answer" }));

    expect(respondToAsk).toHaveBeenCalledTimes(1);
    expect(respondToAsk).toHaveBeenCalledWith("sess-42", "Postgres");
    // Dialog closes after submit.
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("submits with Cmd/Ctrl+Enter", async () => {
    render(<AskUserDialog />);
    fireAsk({ sessionId: "sess-7", question: "Edge case?" });

    const field = screen.getByRole("textbox", { name: "Your answer" });
    await userEvent.type(field, "yes");
    await userEvent.type(field, "{Meta>}{Enter}{/Meta}");

    expect(respondToAsk).toHaveBeenCalledWith("sess-7", "yes");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("replaces the question and clears the prior answer when a new event arrives", async () => {
    render(<AskUserDialog />);
    fireAsk({ sessionId: "s1", question: "First?" });
    await userEvent.type(screen.getByRole("textbox", { name: "Your answer" }), "draft");

    fireAsk({ sessionId: "s2", question: "Second?" });

    expect(screen.getByText("Second?")).toBeInTheDocument();
    expect(screen.queryByText("First?")).not.toBeInTheDocument();
    // Answer box reset for the new question.
    expect(screen.getByRole("textbox", { name: "Your answer" })).toHaveValue("");
  });

  // -- onAnswered callback ---------------------------------------------------

  it("does not require onAnswered — dialog works when the optional prop is omitted", async () => {
    // Given: AskUserDialog rendered without the optional onAnswered prop
    render(<AskUserDialog />);
    fireAsk({ sessionId: "sess-99", question: "No callback?" });

    // When: the answer is submitted
    await userEvent.type(screen.getByRole("textbox", { name: "Your answer" }), "fine");
    await userEvent.click(screen.getByRole("button", { name: "Send answer" }));

    // Then: respondToAsk is called and dialog closes — no error thrown
    expect(respondToAsk).toHaveBeenCalledWith("sess-99", "fine");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("calls onAnswered with the correct session id after successful submit", async () => {
    // Given: a session with a pending ask_user question and an onAnswered callback
    const onAnswered = vi.fn();
    render(<AskUserDialog onAnswered={onAnswered} />);
    fireAsk({ sessionId: "sess-42", question: "Which DB?" });

    // When: the user submits an answer
    await userEvent.type(screen.getByRole("textbox", { name: "Your answer" }), "Postgres");
    await userEvent.click(screen.getByRole("button", { name: "Send answer" }));

    // Then: onAnswered is called exactly once with the session id
    expect(onAnswered).toHaveBeenCalledTimes(1);
    expect(onAnswered).toHaveBeenCalledWith("sess-42");
  });

  it("calls onAnswered with the correct session id when submitted via keyboard shortcut", async () => {
    // Given: onAnswered callback wired up
    const onAnswered = vi.fn();
    render(<AskUserDialog onAnswered={onAnswered} />);
    fireAsk({ sessionId: "sess-7", question: "Edge case?" });

    // When: Cmd/Ctrl+Enter submits the answer
    const field = screen.getByRole("textbox", { name: "Your answer" });
    await userEvent.type(field, "yes");
    await userEvent.type(field, "{Meta>}{Enter}{/Meta}");

    // Then: onAnswered fires with the session id
    expect(onAnswered).toHaveBeenCalledTimes(1);
    expect(onAnswered).toHaveBeenCalledWith("sess-7");
  });

  // -- error handling --------------------------------------------------------

  it("keeps the dialog open and shows the error message when respondToAsk rejects", async () => {
    // Given: respondToAsk will fail
    respondToAsk.mockRejectedValueOnce(new Error("network error"));
    const onAnswered = vi.fn();
    render(<AskUserDialog onAnswered={onAnswered} />);
    fireAsk({ sessionId: "sess-err", question: "Will this fail?" });

    // When: the user submits
    await userEvent.type(screen.getByRole("textbox", { name: "Your answer" }), "answer");
    await userEvent.click(screen.getByRole("button", { name: "Send answer" }));

    // Then: dialog stays open, error is shown, onAnswered not called
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByText("network error")).toBeInTheDocument();
    expect(onAnswered).not.toHaveBeenCalled();
  });

  it("re-enables the submit button after a failed submit so the user can retry", async () => {
    // Given: first submit fails, second succeeds
    respondToAsk
      .mockRejectedValueOnce(new Error("timeout"))
      .mockResolvedValueOnce(undefined);
    render(<AskUserDialog />);
    fireAsk({ sessionId: "sess-retry", question: "Retry me?" });

    const field = screen.getByRole("textbox", { name: "Your answer" });
    const button = screen.getByRole("button", { name: "Send answer" });

    // First attempt — fails
    await userEvent.type(field, "first try");
    await userEvent.click(button);
    expect(screen.getByText("timeout")).toBeInTheDocument();

    // Button must be re-enabled for retry
    expect(button).not.toBeDisabled();

    // Second attempt — succeeds
    await userEvent.click(button);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("replaces the error and resets submitting state when a new ask_user event arrives mid-submit", async () => {
    // Simulate: Q1 submit is in-flight when Q2 arrives
    let resolveQ1!: () => void;
    respondToAsk.mockImplementationOnce(
      () => new Promise<void>((res) => { resolveQ1 = res; })
    );
    render(<AskUserDialog />);
    fireAsk({ sessionId: "s1", question: "Q1?" });

    await userEvent.type(screen.getByRole("textbox", { name: "Your answer" }), "a1");
    await userEvent.click(screen.getByRole("button", { name: "Send answer" }));
    // Q1 is now in-flight (button disabled)
    expect(screen.getByRole("button", { name: "Sending…" })).toBeDisabled();

    // Q2 arrives before Q1 resolves
    fireAsk({ sessionId: "s1", question: "Q2?" });

    // Dialog should show Q2, submit button re-enabled
    expect(screen.getByText("Q2?")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send answer" })).not.toBeDisabled();

    // Q1's resolution must be ignored (nonce mismatch)
    act(() => { resolveQ1(); });
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });
});
