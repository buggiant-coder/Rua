import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DeviceDto } from "@/api/modules/sync";
import CreateProjectDialog from "./CreateProjectDialog";

const api = vi.hoisted(() => ({ browseDirectory: vi.fn(), create: vi.fn() }));
vi.mock("@/api/modules/projects", () => ({ projectsApi: api }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
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
      onOk,
      okText,
      okButtonProps,
    }: {
      children: React.ReactNode;
      footer: React.ReactNode;
      onOk?: () => void;
      okText?: string;
      okButtonProps?: Record<string, unknown>;
    }) => (
      <div>
        {children}
        {footer}
        <button onClick={onOk} {...okButtonProps}>
          {okText}
        </button>
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
        onViewDuplicate={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByText("projects.addFolder"));
    fireEvent.click(screen.getByText("Browse shared storage"));
    await screen.findByText("projects.storagePermissionTitle");
    expect(api.browseDirectory).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("projects.grantStoragePermission"));
    expect(window.MobileStorage.requestAllFilesAccess).toHaveBeenCalledOnce();
    fireEvent(window, new Event("rua:foreground"));
    expect(screen.getByTestId("retry-path")).toHaveTextContent("~");
    granted = true;
    fireEvent(window, new Event("rua:foreground"));
    await waitFor(() =>
      expect(screen.getByTestId("retry-path")).toHaveTextContent(
        "/storage/emulated/0",
      ),
    );
    expect(
      screen.queryByText("projects.storagePermissionTitle"),
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
        onViewDuplicate={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByText("projects.addFolder"));
    fireEvent.click(screen.getByText("Browse shared storage"));
    await screen.findByText("projects.storagePermissionTitle");
    expect(
      screen.getByText("projects.storagePermissionRemote"),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("projects.grantStoragePermission"),
    ).not.toBeInTheDocument();
    expect(window.MobileStorage.requestAllFilesAccess).not.toHaveBeenCalled();
  });
});

describe("Project resource directory creation", () => {
  it("creates a named Project with zero resources from an unpaired device", async () => {
    const outcome = { project: { project_id: "new" }, overlap_warnings: [] };
    api.create.mockResolvedValue(outcome);
    const created = vi.fn();
    render(
      <CreateProjectDialog
        open
        devices={[]}
        onClose={vi.fn()}
        onCreated={created}
        onViewDuplicate={vi.fn()}
      />,
    );
    fireEvent.change(
      screen.getByRole("textbox", { name: "projects.displayName" }),
      { target: { value: "Photos" } },
    );
    fireEvent.click(screen.getByRole("button", { name: "projects.create" }));
    await waitFor(() => expect(created).toHaveBeenCalledWith(outcome));
    expect(api.create).toHaveBeenCalledWith("Photos", []);
    expect(api.browseDirectory).not.toHaveBeenCalled();
  });
});
