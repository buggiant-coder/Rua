import { useState, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { getApiUrl, getApiToken } from "../api/config";

interface CommandEntry {
  name: string;
  scope: string;
  help: string;
  args_hint?: string;
  agent_required: boolean;
  alias_of?: string;
  subsystem?: string;
}

interface CommandSuggestion {
  command: string;
  value: string;
  description: string;
  subsystem?: string;
}

export function useSlashCommands(): CommandSuggestion[] {
  const { t } = useTranslation();
  const [commands, setCommands] = useState<CommandSuggestion[]>([]);

  useEffect(() => {
    const controller = new AbortController();

    async function fetchCommands() {
      try {
        const token = getApiToken();
        const headers: Record<string, string> = {
          "Content-Type": "application/json",
        };
        if (token) {
          headers["Authorization"] = `Bearer ${token}`;
        }

        const response = await fetch(getApiUrl("/commands"), {
          headers,
          signal: controller.signal,
        });

        if (!response.ok) {
          return;
        }

        const data: CommandEntry[] = await response.json();
        const suggestions: CommandSuggestion[] = data.map((entry) => ({
          command: `/${entry.name}`,
          value: entry.name,
          description: t(`chat.commands.${entry.name}.description`, {
            defaultValue: entry.help,
          }),
          subsystem: entry.subsystem,
        }));
        setCommands(suggestions);
      } catch {}
    }

    fetchCommands();
    return () => controller.abort();
  }, [t]);

  return commands;
}
