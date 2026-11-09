import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ProviderInfo } from "../../../../../api/types";

import { ProviderCard } from "./ProviderCard";

vi.mock("./LocalProviderCard", () => ({
  LocalProviderCard: ({ provider }: { provider: ProviderInfo }) => (
    <article data-testid="local-card">{provider.id}</article>
  ),
}));

vi.mock("./RemoteProviderCard", () => ({
  RemoteProviderCard: ({ provider }: { provider: ProviderInfo }) => (
    <article data-testid="remote-card">{provider.id}</article>
  ),
}));

function provider(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
  return {
    id: "dashscope",
    name: "DashScope",
    api_key_prefix: "",
    chat_model: "",
    models: [],
    extra_models: [],
    is_custom: false,
    is_local: false,
    support_model_discovery: false,
    support_connection_check: false,
    freeze_url: false,
    require_api_key: true,
    api_key: "",
    base_url: "",
    generate_kwargs: {},
    ...overrides,
  };
}

describe("ProviderCard", () => {
  it("routes rua-local id payloads to the local provider card even without is_local", () => {
    render(
      <ProviderCard
        provider={provider({ id: "rua-local", name: "Rua Local" })}
        activeModels={null}
        onSaved={vi.fn()}
        isHover={false}
        onMouseEnter={vi.fn()}
        onMouseLeave={vi.fn()}
      />,
    );

    expect(screen.getByTestId("local-card")).toHaveTextContent("rua-local");
    expect(screen.queryByTestId("remote-card")).not.toBeInTheDocument();
  });
});
