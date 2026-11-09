import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const {
  mockGetActiveModels,
  mockListProviderTemplates,
  mockListProviders,
  mockSetActiveLlm,
  mockToastError,
  mockUpdateProjectChat,
  mockUpdateChat,
  storeState,
} = vi.hoisted(() => ({
  mockGetActiveModels: vi.fn(),
  mockListProviderTemplates: vi.fn(),
  mockListProviders: vi.fn(),
  mockSetActiveLlm: vi.fn(),
  mockToastError: vi.fn(),
  mockUpdateProjectChat: vi.fn(),
  mockUpdateChat: vi.fn(),
  storeState: {
    pathname: "/chat",
    selectedAgent: "default",
  },
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
  }),
}));

vi.mock("react-router-dom", () => ({
  useLocation: () => ({ pathname: storeState.pathname }),
}));

vi.mock("../../../api/modules/provider", () => ({
  providerApi: {
    getActiveModels: mockGetActiveModels,
    listProviderTemplates: mockListProviderTemplates,
    listProviders: mockListProviders,
    setActiveLlm: mockSetActiveLlm,
  },
}));

vi.mock("../../../api/modules/chat", () => ({
  chatApi: {
    updateChat: mockUpdateChat,
  },
}));

vi.mock("../../../api/modules/projectChats", () => ({
  projectChatsApi: {
    updateChat: mockUpdateProjectChat,
  },
}));

vi.mock("../../../stores/agentStore", () => ({
  useAgentStore: () => ({ selectedAgent: storeState.selectedAgent }),
}));

vi.mock("@/ui", async () => {
  const actual = await vi.importActual<typeof import("@/ui")>("@/ui");
  return {
    ...actual,
    toast: {
      error: mockToastError,
    },
  };
});

import ModelSelector from "./index";
import {
  resolveOrCreateDraftConversationId,
  loadDraftConversationModel,
  saveDraftConversationModel,
} from "../conversationLifecycle";

const provider = {
  id: "dashscope",
  name: "DashScope",
  api_key: "sk-test",
  require_api_key: true,
  is_custom: false,
  base_url: "",
  models: [
    { id: "qwen-plus", name: "Qwen Plus" },
    { id: "qwen-max", name: "Qwen Max" },
  ],
  extra_models: [{ id: "qwen-extra", name: "Qwen Extra" }],
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((innerResolve) => {
    resolve = innerResolve;
  });
  return { promise, resolve };
}

async function openModelList(activeLabel: string) {
  fireEvent.click(await screen.findByText(activeLabel));
  fireEvent.click(
    await screen.findByRole("button", {
      name: `modelSelector.model: ${activeLabel}`,
    }),
  );
}

async function selectModel(activeLabel: string, modelLabel: string) {
  await openModelList(activeLabel);
  const providerButton = await screen.findByRole("button", {
    name: /DashScope/,
  });
  if (providerButton.getAttribute("aria-expanded") !== "true") {
    fireEvent.click(providerButton);
  }
  fireEvent.click(await screen.findByRole("button", { name: modelLabel }));
}

describe("ModelSelector", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    sessionStorage.clear();
    storeState.pathname = "/chat";
    storeState.selectedAgent = "default";
    mockListProviderTemplates.mockResolvedValue([]);
    mockListProviders.mockResolvedValue([provider]);
    mockGetActiveModels.mockResolvedValue({
      default_llm: { provider_id: "dashscope", model: "qwen-plus" },
    });
    mockSetActiveLlm.mockResolvedValue({});
    mockUpdateChat.mockResolvedValue({});
    mockUpdateProjectChat.mockResolvedValue({});
  });

  it.each(["project:other-project", "independent-agent"])(
    "carries the previous conversation model into a %s draft",
    async (scope) => {
      const model = {
        provider_id: "dashscope",
        model: "qwen-plus",
        thinking_strength: "high",
      };
      const { unmount } = render(
        <ModelSelector chatId="previous" chatModel={model} />,
      );
      await screen.findByText("Qwen Plus");
      unmount();
      const id = resolveOrCreateDraftConversationId(scope);
      expect(loadDraftConversationModel(scope, id)).toEqual(model);
      render(
        <ModelSelector
          chatId={id}
          draftSelection
          chatModel={loadDraftConversationModel(scope, id)}
        />,
      );
      await screen.findByText("Qwen Plus");
      const ownChoice = { provider_id: "dashscope", model: "qwen-max" };
      saveDraftConversationModel(scope, id, ownChoice);
      expect(resolveOrCreateDraftConversationId(scope)).toBe(id);
      expect(loadDraftConversationModel(scope, id)).toEqual(ownChoice);
    },
  );

  it("loads providers and the effective active model for the selected agent", async () => {
    render(<ModelSelector />);

    expect(await screen.findByText("Qwen Plus")).toBeInTheDocument();
    expect(mockListProviders).toHaveBeenCalledOnce();
    expect(mockGetActiveModels).toHaveBeenCalledWith({
      scope: "effective",
      agent_id: "default",
    });
  });

  it("expands a provider inline in the model panel", async () => {
    render(<ModelSelector />);

    await openModelList("Qwen Plus");
    const providerItem = await screen.findByRole("button", {
      name: /DashScope/,
    });

    if (providerItem.getAttribute("aria-expanded") === "true") {
      fireEvent.click(providerItem);
    }
    expect(providerItem).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(providerItem);
    expect(providerItem).toHaveAttribute("aria-expanded", "true");
    expect(
      await screen.findByRole("button", { name: "Qwen Max" }),
    ).toBeVisible();
  });

  it("falls back to the select-model label when there is no active model", async () => {
    mockGetActiveModels.mockResolvedValueOnce({ default_llm: undefined });

    render(<ModelSelector />);

    expect(
      await screen.findByText("modelSelector.selectModel"),
    ).toBeInTheDocument();
  });

  it("shows a bare model id when the active model is not in an eligible provider", async () => {
    mockListProviders.mockResolvedValueOnce([
      { ...provider, api_key: "", models: provider.models },
    ]);

    render(<ModelSelector />);

    expect(await screen.findByText("qwen-plus")).toBeInTheDocument();
  });

  it("treats local providers with models as selectable without api key or base url", async () => {
    mockListProviders.mockResolvedValueOnce([
      {
        ...provider,
        id: "rua-local",
        name: "Rua Local",
        api_key: "",
        base_url: "",
        require_api_key: false,
        is_local: true,
        models: [{ id: "qwen-local", name: "Qwen Local" }],
        extra_models: [],
      },
    ]);
    mockGetActiveModels.mockResolvedValueOnce({
      default_llm: { provider_id: "rua-local", model: "qwen-local" },
    });

    render(<ModelSelector />);

    expect(await screen.findByText("Qwen Local")).toBeInTheDocument();
  });

  it("treats rua-local id payloads as selectable when is_local is stale", async () => {
    mockListProviders.mockResolvedValueOnce([
      {
        ...provider,
        id: "rua-local",
        name: "Rua Local",
        api_key: "",
        base_url: "",
        require_api_key: true,
        is_local: false,
        models: [{ id: "qwen-local", name: "Qwen Local" }],
        extra_models: [],
      },
    ]);
    mockGetActiveModels.mockResolvedValueOnce({
      default_llm: { provider_id: "rua-local", model: "qwen-local" },
    });

    render(<ModelSelector />);

    expect(await screen.findByText("Qwen Local")).toBeInTheDocument();
  });

  it("selects a model at agent scope and emits model-switched", async () => {
    const eventSpy = vi.fn();
    window.addEventListener("model-switched", eventSpy);
    render(<ModelSelector />);

    await selectModel("Qwen Plus", "Qwen Max");

    await waitFor(() =>
      expect(mockSetActiveLlm).toHaveBeenCalledWith({
        provider_id: "dashscope",
        model: "qwen-max",
        scope: "agent",
        agent_id: "default",
      }),
    );
    expect(eventSpy).toHaveBeenCalledOnce();

    window.removeEventListener("model-switched", eventSpy);
  });

  it("persists model selections after a draft becomes an established Chat", async () => {
    const onChatModelChange = vi.fn();
    const confirmedModel = { provider_id: "dashscope", model: "qwen-plus" };
    const { rerender } = render(
      <ModelSelector
        chatId="lazy-chat-uuid"
        chatModel={confirmedModel}
        draftSelection
        onChatModelChange={onChatModelChange}
      />,
    );

    rerender(
      <ModelSelector
        chatId="lazy-chat-uuid"
        chatModel={confirmedModel}
        draftSelection={false}
        onChatModelChange={onChatModelChange}
      />,
    );
    mockUpdateChat.mockResolvedValue({
      model: { provider_id: "dashscope", model: "qwen-max" },
    });
    await selectModel("Qwen Plus", "Qwen Max");

    await waitFor(() =>
      expect(mockUpdateChat).toHaveBeenCalledWith(
        "lazy-chat-uuid",
        { model: { provider_id: "dashscope", model: "qwen-max" } },
        "default",
      ),
    );
    expect(onChatModelChange).toHaveBeenCalledWith({
      provider_id: "dashscope",
      model: "qwen-max",
    });
    expect(mockSetActiveLlm).not.toHaveBeenCalled();
  });

  it("commits a Project model only through the Project target", async () => {
    mockUpdateProjectChat.mockResolvedValue({
      model: { provider_id: "dashscope", model: "qwen-max" },
    });
    const onChatModelChange = vi.fn();
    render(
      <ModelSelector
        chatId="chat/one"
        chatModel={{ provider_id: "dashscope", model: "qwen-plus" }}
        target={{
          scope: "project",
          projectId: "project/one",
          agentId: "default",
        }}
        onChatModelChange={onChatModelChange}
      />,
    );

    await selectModel("Qwen Plus", "Qwen Max");

    await waitFor(() =>
      expect(mockUpdateProjectChat).toHaveBeenCalledWith(
        "project/one",
        "chat/one",
        {
          model: { provider_id: "dashscope", model: "qwen-max" },
        },
      ),
    );
    expect(mockUpdateChat).not.toHaveBeenCalled();
    expect(onChatModelChange).toHaveBeenCalledWith({
      provider_id: "dashscope",
      model: "qwen-max",
    });
  });

  it("keeps the committed Project model when authority validation fails", async () => {
    mockUpdateProjectChat.mockRejectedValueOnce(
      new Error("thinking strength is not supported"),
    );
    const onChatModelChange = vi.fn();
    render(
      <ModelSelector
        chatId="chat/one"
        chatModel={{ provider_id: "dashscope", model: "qwen-plus" }}
        target={{
          scope: "project",
          projectId: "project/one",
          agentId: "default",
        }}
        onChatModelChange={onChatModelChange}
      />,
    );

    await selectModel("Qwen Plus", "Qwen Max");

    await waitFor(() =>
      expect(mockToastError).toHaveBeenCalledWith(
        "thinking strength is not supported",
      ),
    );
    expect(onChatModelChange).not.toHaveBeenCalled();
    expect(screen.getAllByText("Qwen Plus").length).toBeGreaterThan(0);
    expect(mockSetActiveLlm).not.toHaveBeenCalled();
    expect(mockUpdateChat).not.toHaveBeenCalled();
  });

  it("keeps a Lazy Project model selection with a reserved chatId local to the draft Chat", async () => {
    const onChatModelChange = vi.fn();
    render(
      <ModelSelector
        chatId="lazy-chat-uuid"
        chatModel={{ provider_id: "dashscope", model: "qwen-plus" }}
        draftSelection
        target={{
          scope: "project",
          projectId: "project/one",
          agentId: "default",
        }}
        onChatModelChange={onChatModelChange}
      />,
    );

    await selectModel("Qwen Plus", "Qwen Max");

    await waitFor(() =>
      expect(onChatModelChange).toHaveBeenCalledWith({
        provider_id: "dashscope",
        model: "qwen-max",
      }),
    );
    expect(mockSetActiveLlm).not.toHaveBeenCalled();
    expect(mockUpdateProjectChat).not.toHaveBeenCalled();
    expect(mockUpdateChat).not.toHaveBeenCalled();
  });

  it("does not call setActiveLlm when the selected model is already active", async () => {
    render(<ModelSelector />);

    await selectModel("Qwen Plus", "Qwen Plus");

    expect(mockSetActiveLlm).not.toHaveBeenCalled();
  });

  it("prefers a parent-provided chat session model over the agent effective model", async () => {
    render(
      <ModelSelector
        chatId="chat-uuid"
        chatModel={{ provider_id: "dashscope", model: "qwen-max" }}
      />,
    );

    expect(await screen.findByText("Qwen Max")).toBeInTheDocument();
  });

  it("shows the chat model immediately while the provider catalog is loading", async () => {
    const providers = deferred<(typeof provider)[]>();
    mockListProviders.mockReturnValueOnce(providers.promise);

    render(
      <ModelSelector
        chatId="chat-uuid"
        chatModel={{ provider_id: "dashscope", model: "qwen-max" }}
      />,
    );

    expect(screen.getByText("qwen-max")).toBeInTheDocument();

    providers.resolve([provider]);
    expect(await screen.findByText("Qwen Max")).toBeInTheDocument();
  });

  it("does not reload providers for an equivalent chat model snapshot", async () => {
    const { rerender } = render(
      <ModelSelector
        chatId="chat-uuid"
        chatModel={{ provider_id: "dashscope", model: "qwen-max" }}
      />,
    );
    await waitFor(() => expect(mockListProviders).toHaveBeenCalledOnce());

    rerender(
      <ModelSelector
        chatId="chat-uuid"
        chatModel={{ provider_id: "dashscope", model: "qwen-max" }}
      />,
    );

    await waitFor(() => expect(mockListProviders).toHaveBeenCalledOnce());
    expect(mockListProviderTemplates).toHaveBeenCalledOnce();
  });

  it("does not reuse the previous chat model while the next chat model is unresolved", async () => {
    const { rerender } = render(
      <ModelSelector
        chatId="chat-a"
        chatModel={{ provider_id: "dashscope", model: "qwen-plus" }}
      />,
    );
    expect(await screen.findByText("Qwen Plus")).toBeInTheDocument();

    const nextProviders = deferred<(typeof provider)[]>();
    mockListProviders.mockReturnValueOnce(nextProviders.promise);
    rerender(<ModelSelector chatId="chat-b" chatModel={null} />);

    expect(screen.getByText("modelSelector.selectModel")).toBeInTheDocument();
    expect(screen.queryByText("Qwen Plus")).not.toBeInTheDocument();

    nextProviders.resolve([provider]);
  });

  it("does not fall back to the agent active model when chatId has no model", async () => {
    render(<ModelSelector chatId="1700000000000" />);

    expect(
      await screen.findByText("modelSelector.selectModel"),
    ).toBeInTheDocument();
    expect(mockGetActiveModels).not.toHaveBeenCalled();
  });

  it("updates the chat model slot when chatId is provided", async () => {
    const onChatModelChange = vi.fn();
    mockUpdateChat.mockResolvedValueOnce({
      model: {
        provider_id: "dashscope",
        model: "qwen-extra",
        thinking_strength: "auto",
        thinking_param_style: "budget",
      },
    });
    render(
      <ModelSelector
        chatId="chat-uuid"
        onChatModelChange={onChatModelChange}
      />,
    );

    await selectModel("modelSelector.selectModel", "Qwen Extra");

    await waitFor(() =>
      expect(mockUpdateChat).toHaveBeenCalledWith(
        "chat-uuid",
        { model: { provider_id: "dashscope", model: "qwen-extra" } },
        "default",
      ),
    );
    expect(mockSetActiveLlm).not.toHaveBeenCalled();
    expect(onChatModelChange).toHaveBeenCalledWith({
      provider_id: "dashscope",
      model: "qwen-extra",
      thinking_strength: "auto",
      thinking_param_style: "budget",
    });
  });

  it("keeps the current label and reports toast when model switching fails", async () => {
    mockSetActiveLlm.mockRejectedValueOnce(new Error("switch failed"));

    render(<ModelSelector />);

    await selectModel("Qwen Plus", "Qwen Max");

    await waitFor(() =>
      expect(mockToastError).toHaveBeenCalledWith("switch failed"),
    );
    expect(screen.getAllByText("Qwen Plus").length).toBeGreaterThan(0);
  });

  it("updates model thinking strength from the same panel", async () => {
    const onChatModelChange = vi.fn();
    const nextModel = {
      provider_id: "dashscope",
      model: "qwen-plus",
      thinking_strength: "high" as const,
      thinking_enabled: null,
      thinking_param_style: "effort",
      thinking_budget: null,
      reasoning_effort: null,
    };
    mockUpdateChat.mockResolvedValueOnce({ model: nextModel });
    render(
      <ModelSelector
        chatId="chat-reasoning"
        chatModel={{
          provider_id: "dashscope",
          model: "qwen-plus",
          thinking_strength: "auto",
          thinking_param_style: "effort",
        }}
        onChatModelChange={onChatModelChange}
      />,
    );

    fireEvent.click(
      await screen.findByRole("button", {
        name: /modelSelector\.modelAndThinking/,
      }),
    );
    fireEvent.click(
      await screen.findByRole("button", {
        name: "chat.thinkingStrengthOption.high",
      }),
    );

    await waitFor(() =>
      expect(mockUpdateChat).toHaveBeenCalledWith(
        "chat-reasoning",
        { model: nextModel },
        "default",
      ),
    );
    expect(onChatModelChange).toHaveBeenCalledWith(nextModel);
  });

  it("keeps draft thinking strength with a reserved chatId local until the first message", async () => {
    const onChatModelChange = vi.fn();
    render(
      <ModelSelector
        chatId="lazy-chat-uuid"
        chatModel={{
          provider_id: "dashscope",
          model: "qwen-plus",
          thinking_strength: "auto",
        }}
        draftSelection
        onChatModelChange={onChatModelChange}
      />,
    );

    fireEvent.click(
      await screen.findByRole("button", {
        name: /modelSelector\.modelAndThinking/,
      }),
    );
    fireEvent.click(
      await screen.findByRole("button", {
        name: "chat.thinkingStrengthOption.high",
      }),
    );

    expect(onChatModelChange).toHaveBeenCalledWith({
      provider_id: "dashscope",
      model: "qwen-plus",
      thinking_strength: "high",
      thinking_enabled: null,
      thinking_budget: null,
      reasoning_effort: null,
    });
    expect(mockSetActiveLlm).not.toHaveBeenCalled();
    expect(mockUpdateChat).not.toHaveBeenCalled();
  });
});
