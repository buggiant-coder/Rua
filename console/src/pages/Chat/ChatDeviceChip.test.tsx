import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import "@/i18n";
import ChatDeviceChip from "./ChatDeviceChip";

const localDevice = {
  device_id: "3c069ff5c89a4225",
  public_key: "pk-local",
  friendly_name: "mac",
  label: "",
  added_at_ms: 1,
  last_seen_ms: 2,
  is_self: true,
  connectivity_state: "not_connected" as const,
};

describe("ChatDeviceChip", () => {
  it("shows only the local label when the authority is this device", () => {
    render(
      <ChatDeviceChip
        currentDeviceId={localDevice.device_id}
        devices={[localDevice]}
        onSelect={vi.fn()}
      />,
    );

    const chip = screen.getByRole("button", { name: /本机/ });
    expect(chip).toHaveTextContent("本机");
    expect(chip).not.toHaveTextContent("mac");
    expect(chip).not.toHaveAccessibleName(/已连接/);
    expect(chip).toHaveClass("is-local");
    expect(
      document.querySelector(".rua-chat-device-chip-status-dot"),
    ).not.toBeInTheDocument();

    fireEvent.click(chip);
    expect(screen.queryByText(/已连接/)).not.toBeInTheDocument();
    expect(screen.getByText(localDevice.device_id.slice(0, 8))).toBeVisible();
  });

  it("does not expose a raw id as the primary label for an unknown authority", () => {
    render(
      <ChatDeviceChip
        currentDeviceId="47e867abbb41c059"
        devices={[localDevice]}
        onSelect={vi.fn()}
      />,
    );

    const chip = screen.getByRole("button", { name: /未知设备/ });
    expect(chip).toHaveTextContent("未知设备");
    expect(chip).not.toHaveTextContent("47e867ab");

    fireEvent.click(chip);
    expect(screen.getByText(/本机 · mac/)).toBeInTheDocument();
  });

  it("renders the shared Device connectivity state", () => {
    render(
      <ChatDeviceChip
        currentDeviceId="remote-device"
        devices={[
          {
            device_id: "remote-device",
            public_key: "remote-key",
            friendly_name: "Phone",
            label: "Android",
            added_at_ms: 1,
            last_seen_ms: 2,
            is_self: false,
            connectivity_state: "connecting",
          },
        ]}
        onSelect={vi.fn()}
      />,
    );

    const chip = screen.getByRole("button", { name: /Phone/ });
    expect(chip).toHaveClass("is-connecting");
    fireEvent.click(chip);
    expect(screen.getByText(/连接中 · remote-/)).toBeVisible();
  });

  it("does not open the device menu in read-only mode", () => {
    render(
      <ChatDeviceChip
        currentDeviceId={localDevice.device_id}
        devices={[localDevice]}
        readOnly
      />,
    );

    const chip = screen.getByRole("button", { name: /执行设备|mac/ });
    expect(chip).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(chip);
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });
});
