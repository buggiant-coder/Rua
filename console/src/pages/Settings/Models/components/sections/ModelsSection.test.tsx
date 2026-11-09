import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ModelsSection } from "./ModelsSection";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
  }),
}));

vi.mock("../../../../../api", () => ({
  default: {
    setActiveLlm: vi.fn(),
  },
}));

vi.mock("@/ui", async () => {
  const actual = await vi.importActual<typeof import("@/ui")>("@/ui");
  return {
    ...actual,
    toast: {
      success: vi.fn(),
      error: vi.fn(),
    },
  };
});

const localProvider = {
  id: "rua-local",
  name: "Rua Local",
  api_key: "",
  require_api_key: true,
  is_custom: false,
  is_local: false,
  base_url: "",
  models: [{ id: "qwen-local", name: "Qwen Local" }],
  extra_models: [],
};

const remoteMissingKey = {
  id: "dashscope",
  name: "DashScope",
  api_key: "",
  require_api_key: true,
  is_custom: false,
  is_local: false,
  base_url: "",
  models: [{ id: "qwen-plus", name: "Qwen Plus" }],
  extra_models: [],
};

describe("ModelsSection", () => {
  it("offers Rust local provider id payloads without applying remote api-key readiness", async () => {
    render(
      <ModelsSection
        providers={[remoteMissingKey, localProvider]}
        activeModels={null}
        onSaved={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByText("models.selectProvider"));

    expect(await screen.findByText("Rua Local")).toBeInTheDocument();
    expect(screen.queryByText("DashScope")).not.toBeInTheDocument();
  });
});
