import { expect } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

/** Open the settings modal and wait for the dialog to appear. */
export async function openSettingsModal(): Promise<void> {
  const btn = screen.getByRole("button", { name: /settings/i });
  await userEvent.click(btn);
  await waitFor(() => {
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });
}
