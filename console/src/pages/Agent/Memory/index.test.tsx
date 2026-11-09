import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import MemoryPage from "./index";

const { agentsApiMock, memoryApiMock, toastMock, tMock } = vi.hoisted(() => {
  const t = (key: string, params?: Record<string, unknown>) => {
    const values: Record<string, string> = {
      "common.cancel": "Cancel",
      "common.delete": "Delete",
      "common.edit": "Edit",
      "common.confirm": "Confirm",
      "common.more": "More",
      "common.save": "Save",
      "memory.title": "Memory",
      "memory.empty": "No Memory records yet",
      "memory.loadError": "Failed to load Memory",
      "memory.deleteSuccess": "Memory deleted",
      "memory.deleteFailed": "Failed to delete Memory",
      "memory.deleteConfirm": "Delete this Memory record?",
      "memory.reindex": "Reindex",
      "memory.reindexSuccess": `Reindexed ${params?.count ?? 0}`,
      "memory.reindexFailed": "Failed to reindex",
      "memory.searchPlaceholder": "Search Memory",
      "memory.noMatch": "No match",
      "memory.unknownDevice": "Unknown device",
      "memory.device": "Device",
      "memory.updatedAt": "Updated",
      "memory.confidence": "Confidence",
      "memory.importance": "Importance",
      "memory.editTitle": "Edit Memory",
      "memory.memoryTitle": "Title",
      "memory.content": "Fact",
      "memory.searchTerms": "Search terms",
      "memory.kindLabel": "Kind",
      "memory.scopeLabel": "Scope",
      "memory.updateSuccess": "Memory updated",
      "memory.updateFailed": "Failed to update Memory",
      "memory.contentRequired": "Fact content is required",
      "memory.titleRequired": "Title is required",
      "memory.searchTermsRequired": "Search terms are required",
      "memory.confidenceInvalid": "Confidence invalid",
      "memory.importanceInvalid": "Importance invalid",
      "memory.shareAction": "Share",
      "memory.moreActions": "More actions",
      "memory.batchShare": "Batch share",
      "memory.optionsMenu": "Memory options",
      "memory.optionsTitle": "Memory options",
      "memory.optionsLoading": "Loading Memory options...",
      "memory.optionsLoadFailed": "Failed to load Memory options",
      "memory.optionsSaveSuccess": "Memory options saved",
      "memory.optionsSaveFailed": "Failed to save Memory options",
      "memory.searchMode": "Memory search mode",
      "memory.searchModeTooltip": "Search mode help",
      "memory.searchModeGrep": "Brute grep",
      "memory.searchModeGrepDescription":
        "Plain grep over expanded Memory text.",
      "memory.searchModePrecise": "Precise retrieval",
      "memory.searchModePreciseDescription":
        "Two-layer index with terms, resources, vectors, and relations.",
      "agentConfig.memorySummaryEnabled": "Start memory summary",
      "agentConfig.memorySummaryEnabledTooltip":
        "Summarize memory during compaction.",
      "agentConfig.dreamCron": "Dream",
      "agentConfig.dreamCronTooltip": "Schedule memory cleanup.",
      "agentConfig.dreamCronPlaceholder": "0 23 * * *",
      "memory.batchShareTitle": `Share ${params?.count ?? 0} Memory records`,
      "memory.shareTitle": "Memory Visibility",
      "memory.private": "Private",
      "memory.sharedSelected": "Selected agents",
      "memory.sharedAll": "All agents",
      "memory.sharedCount": `Shared with ${params?.count ?? 0}`,
      "memory.selectedCount": `${params?.count ?? 0} selected`,
      "memory.selectRecord": `Select ${params?.title ?? "Memory"}`,
      "memory.batchSelectRequired": "Select at least one Memory record",
      "memory.noShareTargets": "No other agents available",
      "memory.shareSelectAgent": "Select at least one agent",
      "memory.shareSuccess": "Memory visibility updated",
      "memory.batchShareSuccess": `Updated visibility for ${
        params?.count ?? 0
      } Memory records`,
      "memory.shareFailed": "Failed to update Memory visibility",
      "memory.sortUpdatedDesc": "Recently updated",
      "memory.sortCreatedDesc": "Recently created",
      "memory.sortTitleAsc": "Title",
      "memory.kind.fact": "Fact",
      "memory.kind.project_fact": "Project fact",
      "memory.kind.preference": "Preference",
      "memory.kind.convention": "Convention",
      "memory.kind.tool_quirk": "Tool quirk",
      "memory.kind.personal_fact": "Personal fact",
      "memory.scope.user": "User",
      "memory.scope.agent": "Agent",
      "memory.scope.workspace": "Workspace",
      "memory.scope.project": "Project",
      "memory.status.active": "Active",
    };
    return values[key] ?? key;
  };

  return {
    agentsApiMock: {
      getAgentRunningConfig: vi.fn(),
      updateAgentRunningConfig: vi.fn(),
    },
    memoryApiMock: {
      listMemoryRecords: vi.fn(),
      deleteMemoryRecord: vi.fn(),
      updateMemoryRecord: vi.fn(),
      getMemoryVisibilityOptions: vi.fn(),
      updateMemoryVisibility: vi.fn(),
      reindexMemory: vi.fn(),
    },
    toastMock: {
      error: vi.fn(),
      success: vi.fn(),
    },
    tMock: t,
  };
});

vi.mock("@/api/modules/memory", () => ({
  memoryApi: memoryApiMock,
}));

vi.mock("@/api/modules/agents", () => ({
  agentsApi: agentsApiMock,
}));

vi.mock("@/stores/agentStore", () => ({
  useAgentStore: () => ({ selectedAgent: "memory-agent" }),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: tMock,
  }),
}));

vi.mock("@/ui", async () => {
  const actual = await vi.importActual<typeof import("@/ui")>("@/ui");
  return {
    ...actual,
    Popconfirm: ({
      children,
      onConfirm,
    }: {
      children: ReactNode;
      onConfirm?: () => void | Promise<void>;
    }) => (
      <span onClick={() => void onConfirm?.()} data-testid="popconfirm">
        {children}
      </span>
    ),
    toast: toastMock,
  };
});

describe("MemoryPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    agentsApiMock.getAgentRunningConfig.mockReset();
    agentsApiMock.updateAgentRunningConfig.mockReset();
    memoryApiMock.listMemoryRecords.mockReset();
    memoryApiMock.deleteMemoryRecord.mockReset();
    memoryApiMock.updateMemoryRecord.mockReset();
    memoryApiMock.getMemoryVisibilityOptions.mockReset();
    memoryApiMock.updateMemoryVisibility.mockReset();
    memoryApiMock.reindexMemory.mockReset();
    agentsApiMock.getAgentRunningConfig.mockResolvedValue({
      max_iters: 100,
      auto_continue_on_text_only: false,
      llm_retry_enabled: true,
      llm_max_retries: 3,
      llm_backoff_base: 1,
      llm_backoff_cap: 30,
      llm_max_concurrent: 5,
      llm_max_qpm: 0,
      llm_rate_limit_pause: 30,
      llm_rate_limit_jitter: 0,
      llm_acquire_timeout: 120,
      max_input_length: 131_072,
      history_max_length: 10_000,
      context_compact: {
        token_count_model: "default",
        token_count_use_mirror: false,
        token_count_estimate_divisor: 4,
        context_compact_enabled: true,
        blocking_compact_ratio: 0.95,
        early_background_compaction_enabled: false,
        early_compaction_target_ratio: 0.5,
        context_reserve_ratio: 0.1,
        compact_with_thinking_block: true,
        thinking_turns_to_keep: 1,
        min_keep_messages: 5,
      },
      tool_result_compact: {
        enabled: true,
        projection_truncation_enabled: false,
        recent_n: 2,
        old_max_bytes: 4000,
        execution_output_max_tokens: 10_000,
        retention_days: 5,
      },
      memory_summary: {
        memory_summary_enabled: true,
        dream_cron: "0 23 * * *",
        search_mode: "precise",
      },
      embedding_config: {
        backend: "",
        api_key: "",
        base_url: "",
        model_name: "",
        dimensions: 1024,
        enable_cache: true,
        use_dimensions: true,
        max_cache_size: 1000,
        max_input_length: 8192,
        max_batch_size: 16,
      },
      memory_manager_backend: "remelight",
    });
    agentsApiMock.updateAgentRunningConfig.mockImplementation(
      async (_agentId: string, config: unknown) => config,
    );
    memoryApiMock.listMemoryRecords.mockResolvedValue({
      records: [
        {
          id: "mem_1",
          agent_id: "memory-agent",
          scope: "project",
          kind: "project_fact",
          title: "Work documents location",
          content: "Work documents are in D:\\\\Work\\\\Docs.",
          status: "active",
          confidence: 0.9,
          created_at: "2026-06-27T00:00:00Z",
          updated_at: "2026-06-27T00:01:00Z",
          written_on_device_id: "device-pc",
          written_on_device_name_snapshot: "Desktop PC",
          resource_device_id: "device-pc",
          resource_device_name_snapshot: "Desktop PC",
          sources: [],
          links: [],
          terms: [
            {
              term: "work docs",
              normalized_term: "work docs",
              term_kind: "search_term",
              source: "agent",
              created_at: "2026-06-27T00:01:00Z",
            },
          ],
        },
      ],
      visibility: {
        mem_1: {
          memory_id: "mem_1",
          owner_agent_id: "memory-agent",
          mode: "private",
          grantee_agent_ids: [],
          grants: [],
        },
      },
    });
    memoryApiMock.deleteMemoryRecord.mockResolvedValue({ deleted: true });
    memoryApiMock.updateMemoryRecord.mockImplementation((_agentId, _id, body) =>
      Promise.resolve({
        id: "mem_1",
        agent_id: "memory-agent",
        status: "active",
        created_at: "2026-06-27T00:00:00Z",
        updated_at: "2026-06-27T00:02:00Z",
        written_on_device_id: "device-pc",
        written_on_device_name_snapshot: "Desktop PC",
        resource_device_id: "device-pc",
        resource_device_name_snapshot: "Desktop PC",
        sources: [],
        links: [],
        ...body,
      }),
    );
    memoryApiMock.getMemoryVisibilityOptions.mockResolvedValue({
      agents: [
        { id: "agent-b", name: "Research Agent", enabled: true },
        { id: "agent-c", name: "Ops Agent", enabled: true },
      ],
    });
    memoryApiMock.updateMemoryVisibility.mockImplementation(
      (_agentId, memoryId, body) =>
        Promise.resolve({
          memory_id: memoryId,
          owner_agent_id: "memory-agent",
          mode: body.mode,
          grantee_agent_ids: body.grantee_agent_ids ?? [],
          grants: [],
        }),
    );
    memoryApiMock.reindexMemory.mockResolvedValue({ documents_indexed: 1 });
  });

  afterEach(() => {
    cleanup();
  });

  it("renders current-agent memory records with device context and deletes them", async () => {
    render(<MemoryPage />);

    await screen.findByText("Work documents location");
    expect(
      screen.getByText("Work documents are in D:\\\\Work\\\\Docs."),
    ).toBeInTheDocument();
    expect(screen.getByText("Desktop PC")).toBeInTheDocument();
    expect(memoryApiMock.listMemoryRecords).toHaveBeenCalledWith(
      "memory-agent",
    );

    fireEvent.click(screen.getByLabelText("Edit"));
    fireEvent.change(
      screen.getByDisplayValue("Work documents are in D:\\\\Work\\\\Docs."),
      {
        target: { value: "Work docs are under D:\\\\Docs." },
      },
    );
    fireEvent.click(screen.getByText("Save"));

    await waitFor(() =>
      expect(memoryApiMock.updateMemoryRecord).toHaveBeenCalledWith(
        "memory-agent",
        "mem_1",
        expect.objectContaining({
          content: "Work docs are under D:\\\\Docs.",
          title: "Work documents location",
          search_terms: ["work docs"],
          kind: "project_fact",
          scope: "project",
        }),
      ),
    );

    fireEvent.click(screen.getByLabelText("Delete"));

    await waitFor(() =>
      expect(memoryApiMock.deleteMemoryRecord).toHaveBeenCalledWith(
        "memory-agent",
        "mem_1",
      ),
    );
    expect(toastMock.success).toHaveBeenCalledWith("Memory deleted");
  });

  it("updates per-record memory visibility for selected agents", async () => {
    render(<MemoryPage />);

    await screen.findByText("Work documents location");
    fireEvent.click(screen.getByLabelText("Share"));

    await screen.findByText("Memory Visibility");
    fireEvent.click(screen.getByLabelText("Selected agents"));
    fireEvent.click(screen.getByLabelText(/Research Agent/));
    fireEvent.click(screen.getByText("Save"));

    await waitFor(() =>
      expect(memoryApiMock.updateMemoryVisibility).toHaveBeenCalledWith(
        "memory-agent",
        "mem_1",
        {
          mode: "selected",
          grantee_agent_ids: ["agent-b"],
        },
      ),
    );
    expect(toastMock.success).toHaveBeenCalledWith("Memory visibility updated");
    await screen.findByText("Shared with 1");
  });

  it("enters batch share mode and updates selected memory records together", async () => {
    memoryApiMock.listMemoryRecords.mockResolvedValueOnce({
      records: [
        {
          id: "mem_1",
          agent_id: "memory-agent",
          scope: "project",
          kind: "project_fact",
          title: "Work documents location",
          content: "Work documents are in D:\\\\Work\\\\Docs.",
          status: "active",
          confidence: 0.9,
          created_at: "2026-06-27T00:00:00Z",
          updated_at: "2026-06-27T00:01:00Z",
          written_on_device_id: "device-pc",
          written_on_device_name_snapshot: "Desktop PC",
          resource_device_id: "device-pc",
          resource_device_name_snapshot: "Desktop PC",
          sources: [],
          links: [],
          terms: [],
        },
        {
          id: "mem_2",
          agent_id: "memory-agent",
          scope: "agent",
          kind: "fact",
          title: "Path notes location",
          content: "Path notes are stored under ~/Notes.",
          status: "active",
          confidence: 0.8,
          created_at: "2026-06-27T00:00:00Z",
          updated_at: "2026-06-27T00:03:00Z",
          sources: [],
          links: [],
          terms: [],
        },
      ],
      visibility: {
        mem_1: {
          memory_id: "mem_1",
          owner_agent_id: "memory-agent",
          mode: "private",
          grantee_agent_ids: [],
          grants: [],
        },
        mem_2: {
          memory_id: "mem_2",
          owner_agent_id: "memory-agent",
          mode: "private",
          grantee_agent_ids: [],
          grants: [],
        },
      },
    });

    render(<MemoryPage />);

    await screen.findByText("Path notes location");
    fireEvent.click(screen.getByLabelText("More"));
    fireEvent.click(await screen.findByText("Batch share"));

    fireEvent.click(screen.getByLabelText("Select Work documents location"));
    fireEvent.click(screen.getByLabelText("Select Path notes location"));
    expect(screen.getByText("2 selected")).toBeInTheDocument();

    fireEvent.click(screen.getByText("Confirm"));
    await screen.findByText("Share 2 Memory records");
    fireEvent.click(screen.getByLabelText(/Research Agent/));
    fireEvent.click(screen.getByText("Save"));

    await waitFor(() => {
      expect(memoryApiMock.updateMemoryVisibility).toHaveBeenCalledWith(
        "memory-agent",
        "mem_1",
        {
          mode: "selected",
          grantee_agent_ids: ["agent-b"],
        },
      );
      expect(memoryApiMock.updateMemoryVisibility).toHaveBeenCalledWith(
        "memory-agent",
        "mem_2",
        {
          mode: "selected",
          grantee_agent_ids: ["agent-b"],
        },
      );
    });
    expect(toastMock.success).toHaveBeenCalledWith(
      "Updated visibility for 2 Memory records",
    );
  });

  it("saves memory search mode from memory options", async () => {
    render(<MemoryPage />);

    await screen.findByText("Work documents location");
    fireEvent.click(screen.getByLabelText("More"));
    fireEvent.click(await screen.findByText("Memory options"));

    await screen.findByText("Memory search mode");
    fireEvent.click(screen.getByLabelText(/Brute grep/));
    fireEvent.click(screen.getByText("Save"));

    await waitFor(() =>
      expect(agentsApiMock.updateAgentRunningConfig).toHaveBeenCalledWith(
        "memory-agent",
        expect.objectContaining({
          memory_summary: expect.objectContaining({
            search_mode: "grep",
          }),
        }),
      ),
    );
    expect(toastMock.success).toHaveBeenCalledWith("Memory options saved");
  });
});
