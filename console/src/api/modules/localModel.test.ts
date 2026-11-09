import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { localModelApi } from "./localModel";

describe("localModelApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses Rust local-model status and runtime routes", async () => {
    await localModelApi.getLocalServerStatus();
    await localModelApi.getLocalServerUpdateStatus();
    await localModelApi.startLocalServer({ model_id: "qwen2.5" });
    await localModelApi.stopLocalServer();

    expect(request).toHaveBeenNthCalledWith(1, "/local-models/server");
    expect(request).toHaveBeenNthCalledWith(2, "/local-models/server/update");
    expect(request).toHaveBeenNthCalledWith(3, "/local-models/server", {
      method: "POST",
      body: JSON.stringify({ model_id: "qwen2.5" }),
    });
    expect(request).toHaveBeenNthCalledWith(4, "/local-models/server", {
      method: "DELETE",
    });
  });

  it("starts the llama-server binary download with Rust's required model_id body", async () => {
    await localModelApi.startLlamacppDownload();
    await localModelApi.getLlamacppDownloadProgress();
    await localModelApi.cancelLlamacppDownload();

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/local-models/server/download",
      {
        method: "POST",
        body: JSON.stringify({ model_id: "llama-server" }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(2, "/local-models/server/download");
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/local-models/server/download",
      { method: "DELETE" },
    );
  });

  it("reads Rust's recommended catalog separately from downloaded model scans", async () => {
    await localModelApi.listRecommendedLocalModels();

    expect(request).toHaveBeenCalledWith("/local-models/recommended");
  });

  it("starts model downloads with Rust model_id while preserving source as extra metadata", async () => {
    await localModelApi.startLocalModelDownload("qwen2.5:7b", "modelscope");
    await localModelApi.getLocalModelDownloadProgress();
    await localModelApi.cancelLocalModelDownload();

    expect(request).toHaveBeenNthCalledWith(
      1,
      "/local-models/models/download",
      {
        method: "POST",
        body: JSON.stringify({
          model_id: "qwen2.5:7b",
          source: "modelscope",
        }),
      },
    );
    expect(request).toHaveBeenNthCalledWith(2, "/local-models/models/download");
    expect(request).toHaveBeenNthCalledWith(
      3,
      "/local-models/models/download",
      { method: "DELETE" },
    );
  });
});
