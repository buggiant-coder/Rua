import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ImageViewer from "./ImageViewer";

vi.mock("@/api/config", () => ({ getApiUrl: (path: string) => `/api${path}` }));
vi.mock("@/api/authHeaders", () => ({
  buildAuthHeaders: () => ({ Authorization: "Bearer test-token" }),
}));

describe("ImageViewer authenticated media", () => {
  const fetchMock = vi.fn();
  const createObjectURL = vi.fn();
  const revokeObjectURL = vi.fn();
  const src =
    "/api/projects/project-1/chats/chat-1/media/image-1?token=test-token";

  beforeEach(() => {
    vi.stubGlobal("fetch", fetchMock);
    vi.stubGlobal("URL", URL);
    Object.defineProperty(URL, "createObjectURL", {
      configurable: true,
      value: createObjectURL,
    });
    Object.defineProperty(URL, "revokeObjectURL", {
      configurable: true,
      value: revokeObjectURL,
    });
    fetchMock.mockReset();
    createObjectURL.mockReset().mockReturnValue("blob:authenticated-image");
    revokeObjectURL.mockReset();
  });

  afterEach(() => vi.unstubAllGlobals());

  it("loads the uploaded media with Bearer auth and shares bytes with the enlarged preview", async () => {
    const blob = new Blob(["image"], { type: "image/png" });
    fetchMock.mockResolvedValue({ ok: true, blob: async () => blob });
    const { unmount } = render(<ImageViewer src={src} alt="uploaded image" />);
    await waitFor(() =>
      expect(screen.getByAltText("uploaded image").getAttribute("src")).toBe(
        "blob:authenticated-image",
      ),
    );
    expect(fetchMock).toHaveBeenCalledWith(
      new URL(src.split("?")[0], window.location.href).href,
      expect.objectContaining({
        headers: { Authorization: "Bearer test-token" },
        signal: expect.any(AbortSignal),
        redirect: "error",
      }),
    );
    expect(createObjectURL).toHaveBeenCalledWith(blob);
    fireEvent.click(
      screen.getByRole("button", { name: "Open uploaded image" }),
    );
    expect(screen.getAllByAltText("uploaded image")).toHaveLength(2);
    for (const image of screen.getAllByAltText("uploaded image")) {
      expect(image.getAttribute("src")).toBe("blob:authenticated-image");
    }
    expect(fetchMock).toHaveBeenCalledTimes(1);
    unmount();
    expect(fetchMock.mock.calls[0][1].signal.aborted).toBe(true);
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:authenticated-image");
  });

  it.each([
    "blob:local-preview",
    "data:image/png;base64,aA==",
    "https://example.com/api/image.png",
  ])("does not fetch or attach credentials to %s", (source) => {
    render(<ImageViewer src={source} alt="image" />);
    expect(screen.getByAltText("image").getAttribute("src")).toBe(source);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("does not reuse the previous image when the source changes", async () => {
    fetchMock.mockResolvedValue({
      ok: true,
      blob: async () => new Blob(["image"]),
    });
    const { rerender } = render(<ImageViewer src={src} alt="image" />);
    await waitFor(() => expect(createObjectURL).toHaveBeenCalledTimes(1));
    fetchMock.mockResolvedValue({ ok: false });
    rerender(
      <ImageViewer
        src="/api/agents/default/chats/chat-2/media/image-2"
        alt="image"
      />,
    );
    expect(screen.getByAltText("image").getAttribute("src")).toBeNull();
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:authenticated-image");
    rerender(<ImageViewer src={src} alt="image" />);
    expect(screen.getByAltText("image").getAttribute("src")).toBeNull();
  });

  it("discards a late read after unmount", async () => {
    let finish!: (blob: Blob) => void;
    fetchMock.mockResolvedValue({
      ok: true,
      blob: () =>
        new Promise<Blob>((resolve) => {
          finish = resolve;
        }),
    });
    const { unmount } = render(<ImageViewer src={src} alt="image" />);
    await waitFor(() => expect(finish).toBeDefined());
    unmount();
    await act(async () => finish(new Blob(["late image"])));
    expect(createObjectURL).not.toHaveBeenCalled();
  });
});
