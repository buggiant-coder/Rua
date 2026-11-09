import { create } from "zustand";
import api from "../api";

interface TTSStore {
  providerType: string;
  providerId: string;
  model: string;
  voice: string;
  speed: number;
  loaded: boolean;
  fetchConfig: () => Promise<void>;
}

export const useTTSStore = create<TTSStore>((set) => ({
  providerType: "browser",
  providerId: "",
  model: "tts-1",
  voice: "alloy",
  speed: 1.0,
  loaded: false,

  fetchConfig: async () => {
    try {
      const res = await api.getTTSConfig();
      set({
        providerType: res.tts_provider_type ?? "browser",
        providerId: res.tts_provider_id ?? "",
        model: res.tts_model ?? "tts-1",
        voice: res.tts_voice ?? "alloy",
        speed: res.tts_speed ?? 1.0,
        loaded: true,
      });
    } catch {
      set({ loaded: true });
    }
  },
}));
