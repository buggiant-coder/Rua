import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AliyunConfigModal } from "./AliyunConfigModal";

const { listEnvsMock, saveEnvsMock, toastErrorMock, toastSuccessMock } =
  vi.hoisted(() => ({
    listEnvsMock: vi.fn(),
    saveEnvsMock: vi.fn(),
    toastErrorMock: vi.fn(),
    toastSuccessMock: vi.fn(),
  }));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "common.cancel": "Cancel",
        "common.save": "Save",
        "market.aliyunConfig.accessKeyId": "Access key ID",
        "market.aliyunConfig.accessKeyIdRequired": "Access key ID is required",
        "market.aliyunConfig.accessKeySecret": "Access key secret",
        "market.aliyunConfig.accessKeySecretRequired":
          "Access key secret is required",
        "market.aliyunConfig.saveFailed": "Save failed",
        "market.aliyunConfig.saveSuccess": "Saved",
        "market.aliyunConfig.title": "Aliyun configuration",
      })[key] ?? key,
  }),
}));

vi.mock("@/ui", async () => {
  const React = await vi.importActual<typeof import("react")>("react");
  const TextInput = React.forwardRef<
    HTMLInputElement,
    React.InputHTMLAttributes<HTMLInputElement>
  >((props, ref) => <input ref={ref} {...props} />);
  TextInput.displayName = "TextInput";

  return {
    FormField: ({
      children,
      error,
      label,
    }: {
      children?: ReactNode;
      error?: ReactNode;
      label?: ReactNode;
    }) => (
      <label>
        <span>{label}</span>
        {children}
        {error && <strong>{error}</strong>}
      </label>
    ),
    Modal: ({
      cancelText,
      children,
      onClose,
      onOk,
      open,
      title,
    }: {
      cancelText?: ReactNode;
      children?: ReactNode;
      onClose?: () => void;
      onOk?: () => void;
      open?: boolean;
      title?: ReactNode;
    }) =>
      open ? (
        <section aria-label={String(title)}>
          {children}
          <button type="button" onClick={onOk}>
            Save
          </button>
          <button type="button" onClick={onClose}>
            {cancelText}
          </button>
        </section>
      ) : null,
    TextInput,
    toast: {
      error: toastErrorMock,
      success: toastSuccessMock,
    },
  };
});

vi.mock("../../../../api", () => ({
  default: {
    listEnvs: listEnvsMock,
    saveEnvs: saveEnvsMock,
  },
}));

describe("AliyunConfigModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listEnvsMock.mockResolvedValue([{ key: "KEEP_ME", value: "old" }]);
    saveEnvsMock.mockResolvedValue(undefined);
  });

  it("saves Rust market Aliyun credentials into env storage and preserves existing keys", async () => {
    const onClose = vi.fn();
    const onSuccess = vi.fn();

    render(<AliyunConfigModal open onClose={onClose} onSuccess={onSuccess} />);

    fireEvent.change(document.querySelector('input[name="accessKeyId"]')!, {
      target: { value: "  LTAI123  " },
    });
    fireEvent.change(document.querySelector('input[name="accessKeySecret"]')!, {
      target: { value: "  secret-value  " },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(saveEnvsMock).toHaveBeenCalledWith({
        KEEP_ME: "old",
        ALIBABA_CLOUD_ACCESS_KEY_ID: "LTAI123",
        ALIBABA_CLOUD_ACCESS_KEY_SECRET: "secret-value",
      });
    });
    expect(toastSuccessMock).toHaveBeenCalledWith("Saved");
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onSuccess).toHaveBeenCalledTimes(1);
  });

  it("keeps the modal open and reports save errors", async () => {
    const onClose = vi.fn();
    const onSuccess = vi.fn();
    saveEnvsMock.mockRejectedValueOnce(new Error("env write failed"));

    render(<AliyunConfigModal open onClose={onClose} onSuccess={onSuccess} />);

    fireEvent.change(document.querySelector('input[name="accessKeyId"]')!, {
      target: { value: "LTAI123" },
    });
    fireEvent.change(document.querySelector('input[name="accessKeySecret"]')!, {
      target: { value: "secret-value" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => {
      expect(toastErrorMock).toHaveBeenCalledWith("env write failed");
    });
    expect(onClose).not.toHaveBeenCalled();
    expect(onSuccess).not.toHaveBeenCalled();
  });
});
