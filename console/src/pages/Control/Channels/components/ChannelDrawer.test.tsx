import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ChannelDrawer } from "./ChannelDrawer";

const apiMock = vi.hoisted(() => ({
  updateChannelConfig: vi.fn(),
  reapplyChannel: vi.fn(),
  resetQrcode: vi.fn(),
}));
vi.mock("../../../../api", () => ({ default: apiMock }));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    i18n: { language: "en" },
    t: (key: string, options?: { defaultValue?: string }) =>
      options?.defaultValue ?? key,
  }),
}));
vi.mock("./useChannelQrcode", () => ({
  useChannelQrcode: () => ({ reset: apiMock.resetQrcode }),
}));
vi.mock("@/ui", () => ({
  Drawer: ({ children, footer }: any) => (
    <div>
      {children}
      {footer}
    </div>
  ),
  Btn: ({ children, loading: _loading, variant: _variant, ...props }: any) => (
    <button {...props}>{children}</button>
  ),
  FormField: ({ children }: any) => <div>{children}</div>,
  Toggle: () => <span />,
  InfoBox: () => null,
  Modal: { confirm: vi.fn() },
  toast: { success: vi.fn(), error: vi.fn() },
}));

describe("ChannelDrawer resource application", () => {
  it("retries saved configuration without submitting the form again", async () => {
    apiMock.updateChannelConfig.mockRejectedValueOnce(
      new Error(
        "Channel configuration saved, but resource application failed; retry the saved configuration",
      ),
    );
    apiMock.reapplyChannel.mockResolvedValueOnce({ status: "restarted" });
    const onSaved = vi.fn();
    render(
      <ChannelDrawer
        open
        activeKey="custom-fixture"
        isBuiltin={false}
        schema={null}
        initialValues={{ enabled: true }}
        onClose={vi.fn()}
        onSaved={onSaved}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "common.save" }));
    const retry = await screen.findByRole("button", {
      name: "Reapply saved configuration",
    });
    expect(screen.getByRole("alert")).toHaveTextContent("configuration saved");
    expect(onSaved).not.toHaveBeenCalled();
    fireEvent.click(retry);
    await waitFor(() => expect(onSaved).toHaveBeenCalledTimes(1));
    expect(apiMock.updateChannelConfig).toHaveBeenCalledTimes(1);
    expect(apiMock.reapplyChannel).toHaveBeenCalledWith("custom-fixture");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});
