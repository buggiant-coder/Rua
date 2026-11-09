import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DeviceDto } from "@/api/modules/sync";
import CreateProjectDialog from "./CreateProjectDialog";

const api = vi.hoisted(() => ({ browseDirectory: vi.fn(), create: vi.fn() }));
vi.mock("@/api/modules/projects", () => ({ projectsApi: api }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (_: string, fallback: string) => fallback }),
}));
vi.mock("@/components/DirectoryPicker", () => ({
  default: (props: {
    browse: (path: string) => Promise<unknown>;
    onError: (error: unknown) => void;
    browseRequest?: { path: string; id: number };
    value: string;
    onChange: (value: string) => void;
  }) => (
    <>
      <input
        aria-label="Directory"
        value={props.value}
        onChange={(event) => props.onChange(event.target.value)}
      />
      <button
        onClick={() =>
          void props.browse("/storage/emulated/0").catch(props.onError)
        }
      >
        Browse shared storage
      </button>
      <span data-testid="retry-path">{props.browseRequest?.path}</span>
    </>
  ),
}));
vi.mock("@/ui", async () => {
  const React = await vi.importActual<typeof import("react")>("react");
  const Select = Object.assign(
    ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
    {
      Option: ({ children }: { children: React.ReactNode }) => (
        <div>{children}</div>
      ),
    },
  );
  return {
    Modal: ({
      children,
      footer,
    }: {
      children: React.ReactNode;
      footer: React.ReactNode;
    }) => (
      <div>
        {children}
        {footer}
      </div>
    ),
    Select,
    InfoBox: ({
      title,
      children,
    }: {
      title: string;
      children: React.ReactNode;
    }) => (
      <section>
        <h2>{title}</h2>
        {children}
      </section>
    ),
    FormField: ({ children }: { children: React.ReactNode }) => (
      <div>{children}</div>
    ),
    Btn: ({
      children,
      variant: _variant,
      ...props
    }: React.ButtonHTMLAttributes<HTMLButtonElement> & {
      variant?: string;
    }) => <button {...props}>{children}</button>,
    TextInput: (props: React.InputHTMLAttributes<HTMLInputElement>) => (
      <input {...props} />
    ),
  };
});

afterEach(() => {
  delete window.MobileStorage;
  vi.clearAllMocks();
});
const device = (is_self: boolean) =>
  ({
    device_id: "phone",
    connectivity_state: "connected",
    is_self,
  }) as DeviceDto;

describe("Project storage permission guidance", () => {
  it("opens local Android settings and retries only after access is granted", async () => {
    let granted = false;
    window.MobileStorage = {
      hasAllFilesAccess: () => granted,
      requestAllFilesAccess: vi.fn(),
    };
    render(
      <CreateProjectDialog
        open
        devices={[device(true)]}
        onClose={vi.fn()}
        onCreated={vi.fn()}
        onViewExisting={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByText("Browse shared storage"));
    await screen.findByText("Storage permission required");
    expect(api.browseDirectory).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("Grant access"));
    expect(window.MobileStorage.requestAllFilesAccess).toHaveBeenCalledOnce();
    fireEvent(window, new Event("rua:foreground"));
    expect(screen.getByTestId("retry-path")).toHaveTextContent("");
    granted = true;
    fireEvent(window, new Event("rua:foreground"));
    await waitFor(() =>
      expect(screen.getByTestId("retry-path")).toHaveTextContent(
        "/storage/emulated/0",
      ),
    );
    expect(
      screen.queryByText("Storage permission required"),
    ).not.toBeInTheDocument();
  });

  it("guides remote authorization without opening the host phone settings", async () => {
    window.MobileStorage = {
      hasAllFilesAccess: () => false,
      requestAllFilesAccess: vi.fn(),
    };
    api.browseDirectory.mockRejectedValueOnce(
      new Error("Permission denied (os error 13)"),
    );
    render(
      <CreateProjectDialog
        open
        devices={[device(false)]}
        onClose={vi.fn()}
        onCreated={vi.fn()}
        onViewExisting={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByText("Browse shared storage"));
    await screen.findByText("Storage permission required");
    expect(
      screen.getByText(/on the selected resource device, then retry browsing/),
    ).toBeInTheDocument();
    expect(screen.queryByText("Grant access")).not.toBeInTheDocument();
    expect(window.MobileStorage.requestAllFilesAccess).not.toHaveBeenCalled();
  });
});

describe("Project resource directory creation", () => {
  it("submits multiple directories and the selected primary without losing earlier choices", async () => {
    const onCreated = vi.fn();
    const outcome = { project: { project_id: "multi" }, overlap_warnings: [] };
    api.create.mockResolvedValueOnce(outcome);
    render(
      <CreateProjectDialog
        open
        devices={[device(false)]}
        onClose={vi.fn()}
        onCreated={onCreated}
        onViewDuplicate={vi.fn()}
      />,
    );
    fireEvent.change(screen.getByLabelText("Directory"), {
      target: { value: "/work/app" },
    });
    fireEvent.click(screen.getByText("Add another folder"));
    expect(screen.getByLabelText("Directory")).toHaveValue("");
    fireEvent.change(screen.getByLabelText("Directory"), {
      target: { value: "/work/docs" },
    });
    fireEvent.click(screen.getByRole("radio", { name: "/work/docs" }));
    fireEvent.click(screen.getByRole("button", { name: "Create Project" }));
    await waitFor(() =>
      expect(api.create).toHaveBeenCalledWith(
        "phone",
        ["/work/app", "/work/docs"],
        "app",
        1,
      ),
    );
    expect(onCreated).toHaveBeenCalledWith(outcome);
  });
});
