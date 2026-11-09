import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderInfo } from "../../../../../api/types";
import { LocalProviderCard } from "./LocalProviderCard";

const { tMock } = vi.hoisted(() => ({
  tMock: vi.fn((key: string, options?: Record<string, unknown>) => {
    if (key === "models.modelsCount") return `${options?.count} models`;
    return key;
  }),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: tMock }),
}));

vi.mock("../providerIcon", () => ({
  providerIcon: () => "/rua-local.svg",
}));

vi.mock("../modals/ModelManageModal", () => ({
  ModelManageModal: ({ open }: { open: boolean }) =>
    open ? <div role="dialog">local-model-manager</div> : null,
}));

function localProvider(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
  return {
    id: "rua-local",
    name: "Rua Local",
    api_key_prefix: "",
    chat_model: "",
    models: [],
    extra_models: [],
    is_custom: false,
    is_local: true,
    support_model_discovery: false,
    support_connection_check: false,
    freeze_url: true,
    require_api_key: false,
    api_key: "",
    base_url: "",
    generate_kwargs: {},
    ...overrides,
  };
}

describe("LocalProviderCard", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("marks Rust local providers as available based on downloaded models, not api credentials", () => {
    render(
      <LocalProviderCard
        provider={localProvider({
          models: [{ id: "qwen-local", name: "Qwen Local" }],
          extra_models: [{ id: "custom-local", name: "Custom Local" }],
        })}
        onSaved={vi.fn()}
        isHover={false}
        onMouseEnter={vi.fn()}
        onMouseLeave={vi.fn()}
      />,
    );

    expect(screen.getByText("models.available")).toBeInTheDocument();
    expect(screen.getByText("2 models")).toBeInTheDocument();
    expect(screen.queryByText("models.unavailable")).not.toBeInTheDocument();
  });

  it("keeps local providers without models visible but unavailable", () => {
    render(
      <LocalProviderCard
        provider={localProvider()}
        onSaved={vi.fn()}
        isHover={false}
        onMouseEnter={vi.fn()}
        onMouseLeave={vi.fn()}
      />,
    );

    expect(screen.getByText("models.unavailable")).toBeInTheDocument();
    expect(screen.getByText("models.localDownloadFirst")).toBeInTheDocument();
  });

  it("opens the Rust local model manager from the card action", () => {
    render(
      <LocalProviderCard
        provider={localProvider()}
        onSaved={vi.fn()}
        isHover={false}
        onMouseEnter={vi.fn()}
        onMouseLeave={vi.fn()}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "models.models" }));

    expect(screen.getByRole("dialog")).toHaveTextContent("local-model-manager");
  });
});
