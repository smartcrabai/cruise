import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ImageAttachments } from "./ImageAttachments";

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string) => `tauri-asset://${path}`,
}));

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => {
    throw new Error("not in tauri runtime");
  },
}));

const openDialogMock = vi.fn();
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (...args: unknown[]) => openDialogMock(...args),
}));

afterEach(() => {
  cleanup();
  openDialogMock.mockReset();
});

describe("ImageAttachments", () => {
  it("renders the empty drop-zone hint when no attachments", () => {
    render(<ImageAttachments value={[]} onChange={() => {}} />);
    expect(screen.getByText(/Drop image files here/i)).toBeInTheDocument();
  });

  it("renders one preview tile per attachment", () => {
    render(
      <ImageAttachments
        value={["/tmp/a.png", "/tmp/sub/b.jpg"]}
        onChange={() => {}}
      />,
    );
    expect(screen.getByAltText("a.png")).toBeInTheDocument();
    expect(screen.getByAltText("b.jpg")).toBeInTheDocument();
  });

  it("calls onChange with the remaining items when a preview's × is clicked", async () => {
    const onChange = vi.fn();
    render(
      <ImageAttachments
        value={["/tmp/a.png", "/tmp/b.png"]}
        onChange={onChange}
      />,
    );
    const removeBtn = screen.getByRole("button", { name: /Remove a.png/i });
    await userEvent.click(removeBtn);
    expect(onChange).toHaveBeenCalledWith(["/tmp/b.png"]);
  });

  it("appends picker selections via Browse and dedupes against existing paths", async () => {
    const onChange = vi.fn();
    openDialogMock.mockResolvedValueOnce(["/tmp/a.png", "/tmp/c.png"]);
    render(
      <ImageAttachments value={["/tmp/a.png"]} onChange={onChange} />,
    );
    await userEvent.click(screen.getByRole("button", { name: /Browse/i }));
    await vi.waitFor(() => {
      expect(onChange).toHaveBeenCalledWith(["/tmp/a.png", "/tmp/c.png"]);
    });
  });

  it("ignores non-image selections from Browse", async () => {
    const onChange = vi.fn();
    openDialogMock.mockResolvedValueOnce(["/tmp/notes.txt"]);
    render(<ImageAttachments value={[]} onChange={onChange} />);
    await userEvent.click(screen.getByRole("button", { name: /Browse/i }));
    await vi.waitFor(() => {
      expect(onChange).toHaveBeenCalledWith([]);
    });
  });

  it("does nothing when the dialog is cancelled (null result)", async () => {
    const onChange = vi.fn();
    openDialogMock.mockResolvedValueOnce(null);
    render(<ImageAttachments value={[]} onChange={onChange} />);
    await userEvent.click(screen.getByRole("button", { name: /Browse/i }));
    await new Promise((r) => setTimeout(r, 10));
    expect(onChange).not.toHaveBeenCalled();
  });
});
