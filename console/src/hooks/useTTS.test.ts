import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useTTSStore } from "../stores/ttsStore";
import { useTTS } from "./useTTS";

const browserTTSMock = vi.hoisted(() => ({
  speakWithBrowser: vi.fn(),
  cancelBrowserTTS: vi.fn(),
  loadBrowserTTSConfig: vi.fn(() => ({ enabled: true })),
  isBrowserTTSSupported: vi.fn(() => true),
}));

vi.mock("../utils/browserTTS", () => browserTTSMock);

vi.mock("../utils/isMobile", () => ({
  isMobile: () => false,
}));

describe("useTTS", () => {
  it("does not stop another message playback when an idle hook unmounts", () => {
    const cancel = vi.fn();
    browserTTSMock.speakWithBrowser.mockReturnValue({
      cancel,
      promise: new Promise<void>(() => {}),
    });
    const playing = renderHook(() => useTTS());
    act(() => playing.result.current.speak("keep reading"));
    const idle = renderHook(() => useTTS());
    browserTTSMock.cancelBrowserTTS.mockClear();
    idle.unmount();
    expect(browserTTSMock.cancelBrowserTTS).not.toHaveBeenCalled();
    expect(cancel).not.toHaveBeenCalled();
    playing.unmount();
    expect(cancel).toHaveBeenCalledTimes(1);
  });

  beforeEach(() => {
    vi.clearAllMocks();
    useTTSStore.setState({ providerType: "browser" });
    browserTTSMock.speakWithBrowser.mockReturnValue({
      cancel: vi.fn(),
      promise: Promise.resolve(),
    });
  });

  it("does not speak when the provider is disabled or text is empty", () => {
    useTTSStore.setState({ providerType: "disabled" });
    const { result } = renderHook(() => useTTS());

    expect(result.current.isDisabled).toBe(true);

    act(() => result.current.speak("hello"));
    act(() => result.current.speak(""));

    expect(browserTTSMock.speakWithBrowser).not.toHaveBeenCalled();
  });

  it("routes browser provider playback through browserTTS and clears speaking state", async () => {
    let finish!: () => void;
    browserTTSMock.speakWithBrowser.mockReturnValue({
      cancel: vi.fn(),
      promise: new Promise<void>((resolve) => {
        finish = resolve;
      }),
    });
    const { result } = renderHook(() => useTTS());

    act(() => result.current.speak("read this"));

    expect(browserTTSMock.cancelBrowserTTS).not.toHaveBeenCalled();
    expect(browserTTSMock.speakWithBrowser).toHaveBeenCalledWith({
      text: "read this",
    });
    await waitFor(() => expect(result.current.isSpeaking).toBe(true));

    act(() => finish());
    await waitFor(() => expect(result.current.isSpeaking).toBe(false));
  });

  it("stop cancels browser playback and resets transient state", () => {
    const { result } = renderHook(() => useTTS());

    act(() => result.current.speak("read this"));
    act(() => result.current.stop());

    expect(
      browserTTSMock.speakWithBrowser.mock.results[0].value.cancel,
    ).toHaveBeenCalledTimes(1);
    expect(result.current.isSpeaking).toBe(false);
    expect(result.current.isLoading).toBe(false);
  });
});
