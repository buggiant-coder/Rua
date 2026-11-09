import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../request", () => ({ request: vi.fn() }));

import { request } from "../request";
import { rootApi } from "./root";

describe("rootApi Rust route canaries", () => {
  afterEach(() => vi.clearAllMocks());

  it("uses the root fallback and public version route", async () => {
    await rootApi.readRoot();
    await rootApi.getVersion();

    expect(request).toHaveBeenNthCalledWith(1, "/");
    expect(request).toHaveBeenNthCalledWith(2, "/version");
  });
});
