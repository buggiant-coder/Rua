import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ProviderInfo } from "../../../../../api/types";
import { RemoteProviderCard } from "./RemoteProviderCard";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: Record<string, unknown>) => {
      if (key === "models.modelsCount") return `${options?.count} models`;
      return key;
    },
  }),
}));

vi.mock("../providerIcon", () => ({
  providerIcon: () => "/provider.svg",
}));

vi.mock("../modals/ProviderConfigModal", () => ({
  ProviderConfigModal: () => null,
}));

vi.mock("../modals/ModelManageModal", () => ({
  ModelManageModal: () => null,
}));

function provider(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
  return {
    id: "dashscope",
    name: "DashScope",
    api_key_prefix: "sk-",
    chat_model: "DashScopeChatModel",
    models: [{ id: "qwen-plus", name: "Qwen Plus" }],
    extra_models: [],
    is_custom: false,
    is_local: false,
    support_model_discovery: true,
    support_connection_check: true,
    freeze_url: true,
    require_api_key: true,
    api_key: "sk-***",
    base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
    generate_kwargs: {},
    ...overrides,
  };
}

describe("RemoteProviderCard", () => {
  it("renders provider family and variant metadata when present", () => {
    render(
      <RemoteProviderCard
        provider={provider({
          provider_group: "aliyun",
          provider_group_name: "Aliyun",
          provider_variant: "dashscope",
        })}
        activeModels={null}
        onSaved={vi.fn()}
        isHover={false}
        onMouseEnter={vi.fn()}
        onMouseLeave={vi.fn()}
      />,
    );

    expect(screen.getByText("models.providerFamily")).toBeInTheDocument();
    expect(screen.getByText("Aliyun / dashscope")).toBeInTheDocument();
  });
});
