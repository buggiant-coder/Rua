import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import DirectoryPicker from "./index";

vi.mock("@/ui", async () => {
  const React = await vi.importActual<typeof import("react")>("react");
  return {
    Btn: ({
      children,
      ...props
    }: React.ButtonHTMLAttributes<HTMLButtonElement>) => (
      <button {...props}>{children}</button>
    ),
    TextInput: React.forwardRef<
      HTMLInputElement,
      React.InputHTMLAttributes<HTMLInputElement>
    >((props, ref) => <input ref={ref} {...props} />),
  };
});

describe("DirectoryPicker", () => {
  it("loads an explicit retry request once even when callbacks are recreated", async () => {
    const browse = vi
      .fn()
      .mockResolvedValue({ path: "/storage/emulated/0", directories: [] });
    const request = { path: "/storage/emulated/0", id: 1 };
    const view = render(
      <DirectoryPicker
        value=""
        onChange={vi.fn()}
        browse={browse}
        browseRequest={request}
      />,
    );
    await waitFor(() => expect(browse).toHaveBeenCalledOnce());
    expect(browse).toHaveBeenCalledWith("/storage/emulated/0");
    view.rerender(
      <DirectoryPicker
        value=""
        onChange={vi.fn()}
        browse={browse}
        browseRequest={request}
      />,
    );
    expect(browse).toHaveBeenCalledOnce();
  });
  it("ignores folder creation results after the device picker unmounts", async () => {
    let resolve!: (listing: { path: string; directories: [] }) => void;
    const pending = new Promise<{ path: string; directories: [] }>((done) => {
      resolve = done;
    });
    const onChange = vi.fn();
    const view = render(
      <DirectoryPicker
        value="/work"
        onChange={onChange}
        browse={vi.fn().mockResolvedValue({ path: "/work", directories: [] })}
        createDirectory={() => pending}
      />,
    );
    fireEvent.click(screen.getByLabelText("Browse folders"));
    fireEvent.click(await screen.findByText("New folder"));
    fireEvent.change(screen.getByLabelText("Folder name"), {
      target: { value: "my" },
    });
    fireEvent.click(screen.getByText("New folder"));
    view.unmount();
    resolve({ path: "/work/my", directories: [] });
    await pending;
    expect(onChange).not.toHaveBeenCalledWith("/work/my");
  });
  it("navigates to storage roots supplied by the resource device", async () => {
    const browse = vi.fn().mockResolvedValue({
      path: "/private/home",
      directories: [],
      roots: [{ name: "SD card", path: "/storage/ABCD-1234" }],
    });
    render(<DirectoryPicker value="" onChange={vi.fn()} browse={browse} />);
    fireEvent.click(screen.getByLabelText("Browse folders"));
    fireEvent.click(await screen.findByText("SD card"));
    await waitFor(() =>
      expect(browse).toHaveBeenCalledWith("/storage/ABCD-1234"),
    );
  });

  it("explicitly creates a child folder and selects the returned path", async () => {
    const onChange = vi.fn();
    const browse = vi
      .fn()
      .mockResolvedValue({ path: "/work", parent: "/", directories: [] });
    const createDirectory = vi.fn().mockResolvedValue({
      path: "/work/my",
      parent: "/work",
      directories: [],
    });
    render(
      <DirectoryPicker
        value="/work"
        onChange={onChange}
        browse={browse}
        createDirectory={createDirectory}
      />,
    );
    fireEvent.click(screen.getByLabelText("Browse folders"));
    fireEvent.click(await screen.findByText("New folder"));
    fireEvent.change(screen.getByLabelText("Folder name"), {
      target: { value: "my" },
    });
    expect(createDirectory).not.toHaveBeenCalled();
    fireEvent.keyDown(screen.getByLabelText("Folder name"), { key: "Enter" });
    await waitFor(() =>
      expect(createDirectory).toHaveBeenCalledWith("/work", "my"),
    );
    await waitFor(() => expect(onChange).toHaveBeenCalledWith("/work/my"));
  });

  it("reports folder creation failure without selecting a nonexistent path", async () => {
    const onChange = vi.fn();
    const onError = vi.fn();
    const error = new Error("Already exists");
    render(
      <DirectoryPicker
        value="/work"
        onChange={onChange}
        browse={vi.fn().mockResolvedValue({ path: "/work", directories: [] })}
        createDirectory={vi.fn().mockRejectedValue(error)}
        onError={onError}
      />,
    );
    fireEvent.click(screen.getByLabelText("Browse folders"));
    fireEvent.click(await screen.findByText("New folder"));
    fireEvent.change(screen.getByLabelText("Folder name"), {
      target: { value: "existing" },
    });
    fireEvent.click(screen.getByText("New folder"));
    await waitFor(() => expect(onError).toHaveBeenCalledWith(error));
    expect(onChange).not.toHaveBeenCalledWith("/work/existing");
    expect(screen.getByLabelText("Folder name")).toHaveValue("existing");
  });

  it("opens the device home directory when the path is empty", async () => {
    const browse = vi.fn().mockResolvedValue({
      path: "/Users/tester",
      parent: "/Users",
      directories: [],
    });

    render(
      <DirectoryPicker
        value=""
        onChange={vi.fn()}
        browse={browse}
        openLabel="Browse folders"
      />,
    );

    fireEvent.click(screen.getByLabelText("Browse folders"));

    await waitFor(() => expect(browse).toHaveBeenCalledWith("~"));
  });

  it("suggests matching directories and supports keyboard selection", async () => {
    const onChange = vi.fn();
    const browse = vi.fn().mockResolvedValue({
      path: "/work",
      parent: "/",
      directories: [
        { name: "rua", path: "/work/rua" },
        { name: "notes", path: "/work/notes" },
      ],
    });

    render(
      <DirectoryPicker value="/work/ru" onChange={onChange} browse={browse} />,
    );

    const input = screen.getByRole("textbox");
    fireEvent.focus(input);
    await waitFor(() => expect(screen.getByRole("option")).toBeInTheDocument());
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(onChange).toHaveBeenCalledWith("/work/rua");
  });

  it("opens the browser and navigates into a directory", async () => {
    const onChange = vi.fn();
    const browse = vi
      .fn()
      .mockResolvedValueOnce({
        path: "/work",
        parent: "/",
        directories: [{ name: "rua", path: "/work/rua" }],
      })
      .mockResolvedValueOnce({
        path: "/work/rua",
        parent: "/work",
        directories: [],
      });

    render(
      <DirectoryPicker
        value="/work"
        onChange={onChange}
        browse={browse}
        openLabel="Browse folders"
      />,
    );

    fireEvent.click(screen.getByLabelText("Browse folders"));
    await waitFor(() => expect(screen.getByText("rua")).toBeInTheDocument());
    fireEvent.click(screen.getByText("rua"));

    await waitFor(() => expect(onChange).toHaveBeenCalledWith("/work/rua"));
  });
});
