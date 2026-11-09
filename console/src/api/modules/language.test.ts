import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { languageApi } from "./language";

describe("languageApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses the shared settings language route and JSON body", async () => {
    await languageApi.getLanguage();
    await languageApi.updateLanguage("zh-CN");

    expect(request).toHaveBeenNthCalledWith(1, "/settings/language");
    expect(request).toHaveBeenNthCalledWith(2, "/settings/language", {
      method: "PUT",
      body: JSON.stringify({ language: "zh-CN" }),
    });
  });
});
