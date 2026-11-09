import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "@/i18n";
import { ModalProvider } from "@/ui";
import type { LocalDownloadProgress, LocalServerStatus } from "@/api/types";
import { LocalRuntimePanel } from "./LocalRuntimePanel";

function status(overrides: Partial<LocalServerStatus> = {}): LocalServerStatus {
  return {
    available: true,
    installable: true,
    installed: false,
    port: null,
    model_name: null,
    message: null,
    ...overrides,
  };
}

function progress(
  overrides: Partial<LocalDownloadProgress> = {},
): LocalDownloadProgress {
  return {
    status: "idle",
    model_name: null,
    downloaded_bytes: 0,
    total_bytes: null,
    speed_bytes_per_sec: 0,
    source: null,
    error: null,
    local_path: null,
    ...overrides,
  };
}

function renderPanel(props: {
  serverStatus?: LocalServerStatus | null;
  hasUpdate?: boolean;
  progress?: LocalDownloadProgress | null;
  onStart?: () => void;
  onClose?: () => void;
}) {
  return render(
    <ModalProvider>
      <LocalRuntimePanel
        serverStatus={props.serverStatus ?? status()}
        hasUpdate={props.hasUpdate ?? false}
        progress={props.progress ?? null}
        onStart={props.onStart ?? vi.fn()}
        onClose={props.onClose ?? vi.fn()}
        onStop={vi.fn()}
        stopping={false}
      />
    </ModalProvider>,
  );
}

describe("LocalRuntimePanel readiness states", () => {
  beforeEach(async () => {
    vi.clearAllMocks();
    await i18n.changeLanguage("en");
  });

  it("shows unsupported local runtime and disables install when Rust marks it non-installable", () => {
    const onStart = vi.fn();
    renderPanel({
      serverStatus: status({
        available: false,
        installable: false,
        message: "unsupported platform",
      }),
      onStart,
    });

    expect(screen.getByText("Unsupported")).toBeInTheDocument();
    expect(screen.getByText("Dead")).toBeInTheDocument();

    const install = screen.getByRole("button", { name: "Install llama.cpp" });
    expect(install).toBeDisabled();
    fireEvent.click(install);
    expect(onStart).not.toHaveBeenCalled();
  });

  it("renders installed-but-idle runtime without offering an install action", () => {
    renderPanel({
      serverStatus: status({ installed: true }),
    });

    expect(screen.getByText("Installed")).toBeInTheDocument();
    expect(screen.getByText("Dead")).toBeInTheDocument();
    expect(
      screen.getByText(
        "Please download and start a suitable model from the list below",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Install llama.cpp" }),
    ).not.toBeInTheDocument();
  });

  it("confirms update before starting while a model is currently running", async () => {
    const onStart = vi.fn();
    renderPanel({
      serverStatus: status({
        available: true,
        installed: true,
        model_name: "qwen2.5-coder",
      }),
      hasUpdate: true,
      onStart,
    });

    fireEvent.click(screen.getByRole("button", { name: "Update available" }));

    await screen.findByText("Confirm llama.cpp update");
    expect(
      screen.getByText(/currently running model service \(qwen2\.5-coder\)/),
    ).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));
    await waitFor(() => expect(onStart).toHaveBeenCalledOnce());
  });

  it("shows active download progress and cancel action instead of install/update", () => {
    const onClose = vi.fn();
    renderPanel({
      serverStatus: status({ installed: false }),
      progress: progress({
        status: "downloading",
        downloaded_bytes: 512,
        total_bytes: 1024,
      }),
      onClose,
    });

    expect(screen.getByText("50% · 512.0 B / 1.0 KB")).toBeInTheDocument();
    expect(
      screen.getByText(
        "You can navigate away — the download will continue in the background.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Install llama.cpp" }),
    ).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button"));
    expect(onClose).toHaveBeenCalledOnce();
  });
});
