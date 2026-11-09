import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useChannelQrcode } from "./useChannelQrcode";

const apiMock = vi.hoisted(() => ({
  getChannelQrcode: vi.fn(),
  getChannelQrcodeStatus: vi.fn(),
}));

vi.mock("../../../../api", () => ({
  api: apiMock,
}));

function renderQrcodeHook(
  overrides: Partial<Parameters<typeof useChannelQrcode>[0]> = {},
) {
  const onSuccess = vi.fn();
  const onExpired = vi.fn();
  const onError = vi.fn();
  const hook = renderHook(() =>
    useChannelQrcode({
      channel: "wechat",
      successStatus: "confirmed",
      successCredentialKey: "bot_token",
      pollInterval: 25,
      onSuccess,
      onExpired,
      onError,
      ...overrides,
    }),
  );

  return { ...hook, onSuccess, onExpired, onError };
}

async function fetchAndPollOnce(
  hook: ReturnType<typeof renderQrcodeHook>["result"],
) {
  await act(async () => {
    await hook.current.fetchQrcode();
  });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(25);
  });
}

describe("useChannelQrcode", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.clearAllMocks();
    apiMock.getChannelQrcode.mockResolvedValue({
      qrcode_img: "base64-png",
      poll_token: "poll-token",
    });
    apiMock.getChannelQrcodeStatus.mockResolvedValue({
      status: "pending",
      credentials: {},
    });
  });

  afterEach(() => {
    vi.runOnlyPendingTimers();
    vi.useRealTimers();
  });

  it("fetches a Rust channel QR code and starts polling by channel and token", async () => {
    const { result } = renderQrcodeHook({ channel: "dingtalk" });

    await act(async () => {
      await result.current.fetchQrcode();
    });

    expect(apiMock.getChannelQrcode).toHaveBeenCalledWith(
      "dingtalk",
      undefined,
    );
    expect(result.current.qrcodeImg).toBe("base64-png");
    expect(result.current.loading).toBe(false);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(25);
    });

    expect(apiMock.getChannelQrcodeStatus).toHaveBeenCalledWith(
      "dingtalk",
      "poll-token",
      undefined,
    );
  });

  it("clears the QR code and calls onSuccess once when credentials match the success status", async () => {
    apiMock.getChannelQrcodeStatus.mockResolvedValue({
      status: "confirmed",
      credentials: { bot_token: "secret-token" },
    });
    const { result, onSuccess, onError } = renderQrcodeHook();

    await fetchAndPollOnce(result);

    expect(result.current.qrcodeImg).toBe("");
    expect(onSuccess).toHaveBeenCalledTimes(1);
    expect(onSuccess).toHaveBeenCalledWith({ bot_token: "secret-token" });
    expect(onError).not.toHaveBeenCalled();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    expect(onSuccess).toHaveBeenCalledTimes(1);
    expect(apiMock.getChannelQrcodeStatus).toHaveBeenCalledTimes(1);
  });

  it("treats expired QR status as an expired error and stops polling", async () => {
    apiMock.getChannelQrcodeStatus.mockResolvedValue({
      status: "expired",
      credentials: {},
    });
    const { result, onExpired, onError, onSuccess } = renderQrcodeHook();

    await fetchAndPollOnce(result);

    expect(result.current.qrcodeImg).toBe("");
    expect(onExpired).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenCalledWith("expired");
    expect(onSuccess).not.toHaveBeenCalled();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    expect(apiMock.getChannelQrcodeStatus).toHaveBeenCalledTimes(1);
  });

  it("reports fetch failures and missing QR image without polling", async () => {
    apiMock.getChannelQrcode.mockResolvedValueOnce({
      qrcode_img: "",
      poll_token: "poll-token",
    });
    const first = renderQrcodeHook();
    await act(async () => {
      await first.result.current.fetchQrcode();
    });
    expect(first.onError).toHaveBeenCalledWith("fetch");
    expect(apiMock.getChannelQrcodeStatus).not.toHaveBeenCalled();
    first.unmount();

    apiMock.getChannelQrcode.mockRejectedValueOnce(new Error("unavailable"));
    const second = renderQrcodeHook();
    await act(async () => {
      await second.result.current.fetchQrcode();
    });
    expect(second.onError).toHaveBeenCalledWith("fetch");
    expect(second.result.current.loading).toBe(false);
  });

  it("ignores transient poll failures and continues until stopped", async () => {
    apiMock.getChannelQrcodeStatus
      .mockRejectedValueOnce(new Error("temporary"))
      .mockResolvedValueOnce({
        status: "confirmed",
        credentials: { bot_token: "secret-token" },
      });
    const { result, onSuccess, unmount } = renderQrcodeHook();

    await act(async () => {
      await result.current.fetchQrcode();
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(25);
    });
    expect(onSuccess).not.toHaveBeenCalled();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(25);
    });
    expect(onSuccess).toHaveBeenCalledWith({ bot_token: "secret-token" });

    unmount();
  });

  it("reset and unmount clear pending polling timers", async () => {
    const { result, unmount } = renderQrcodeHook();

    await act(async () => {
      await result.current.fetchQrcode();
    });
    act(() => {
      result.current.reset();
    });
    expect(result.current.qrcodeImg).toBe("");

    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    expect(apiMock.getChannelQrcodeStatus).not.toHaveBeenCalled();

    await act(async () => {
      await result.current.fetchQrcode();
    });
    unmount();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    expect(apiMock.getChannelQrcodeStatus).not.toHaveBeenCalled();
  });
});
