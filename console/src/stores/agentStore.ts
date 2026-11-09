import { create } from "zustand";
import { persist } from "zustand/middleware";
import { agentsApi } from "../api/modules/agents";
import type { AgentSummary } from "../api/types/agents";

interface AgentStore {
  selectedAgent: string;

  agents: AgentSummary[];
  catalogLoaded: boolean;
  catalogLoading: boolean;
  catalogError: string | null;
  setSelectedAgent: (agentId: string) => void;
  setAgents: (agents: AgentSummary[]) => void;
  ensureCatalogLoaded: () => Promise<AgentSummary[]>;
  refreshCatalog: () => Promise<AgentSummary[]>;
  addAgent: (agent: AgentSummary) => void;
  removeAgent: (agentId: string) => void;
  updateAgent: (agentId: string, updates: Partial<AgentSummary>) => void;
}

let catalogRequest: Promise<AgentSummary[]> | null = null;

export const useAgentStore = create<AgentStore>()(
  persist(
    (set, get) => ({
      selectedAgent: "default",
      agents: [],
      catalogLoaded: false,
      catalogLoading: false,
      catalogError: null,

      setSelectedAgent: (agentId) => set({ selectedAgent: agentId }),

      setAgents: (agents) =>
        set({
          agents,
          catalogLoaded: true,
          catalogLoading: false,
          catalogError: null,
        }),

      ensureCatalogLoaded: async () => {
        const state = get();
        if (state.catalogLoaded) return state.agents;
        return state.refreshCatalog();
      },

      refreshCatalog: async () => {
        if (catalogRequest) return catalogRequest;

        set({ catalogLoading: true, catalogError: null });
        catalogRequest = agentsApi
          .listAgents()
          .then((response) => {
            const agents = response.agents;
            set({
              agents,
              catalogLoaded: true,
              catalogLoading: false,
              catalogError: null,
            });
            return agents;
          })
          .catch((error: unknown) => {
            set({
              catalogLoading: false,
              catalogError:
                error instanceof Error ? error.message : String(error),
            });
            throw error;
          })
          .finally(() => {
            catalogRequest = null;
          });

        return catalogRequest;
      },

      addAgent: (agent) =>
        set((state) => ({
          agents: [...state.agents, agent],
        })),

      removeAgent: (agentId) =>
        set((state) => ({
          agents: state.agents.filter((a) => a.id !== agentId),
          ...(state.selectedAgent === agentId
            ? { selectedAgent: "default" }
            : {}),
        })),

      updateAgent: (agentId, updates) =>
        set((state) => ({
          agents: state.agents.map((a) =>
            a.id === agentId ? { ...a, ...updates } : a,
          ),
        })),
    }),
    {
      name: "rua-agent-storage",
      partialize: (state) => ({ selectedAgent: state.selectedAgent }),
      storage: {
        getItem: (name) => {
          try {
            const value = sessionStorage.getItem(name);
            return value ? JSON.parse(value) : null;
          } catch (error) {
            console.error(`Failed to parse agent storage "${name}":`, error);

            sessionStorage.removeItem(name);
            return null;
          }
        },
        setItem: (name, value) => {
          try {
            sessionStorage.setItem(name, JSON.stringify(value));
          } catch (error) {
            console.error(`Failed to save agent storage "${name}":`, error);
          }
        },
        removeItem: (name) => {
          sessionStorage.removeItem(name);
        },
      },
    },
  ),
);
