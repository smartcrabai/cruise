import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { AskUserDialog } from "../components/AskUserDialog";
import { ASK_USER_EVENT, type AskUserDetail } from "../lib/askUser";

// AskUserDialog calls respondToAsk (a Tauri IPC wrapper) on submit; mock it.
const respondToAsk = vi.fn(() => Promise.resolve());
vi.mock("../lib/commands", () => ({
  respondToAsk: (sessionId: string, answer: string) => respondToAsk(sessionId, answer),
}));

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
});
