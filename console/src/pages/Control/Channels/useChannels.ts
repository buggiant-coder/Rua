import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import api from "../../../api";
import type { ChannelSchemasResponse } from "../../../api/types/channel";
import { useAgentStore } from "../../../stores/agentStore";

export interface ChannelsInitialData {
  agentId: string;
  channels: Record<string, Record<string, unknown>>;
  channelTypes: string[];
  schemas?: ChannelSchemasResponse;
}

export function useChannels(initialData?: ChannelsInitialData) {
  const { selectedAgent } = useAgentStore();
  const [channels, setChannels] = useState<
    Record<string, Record<string, unknown>>
  >(initialData?.channels ?? {});
  const [channelTypes, setChannelTypes] = useState<string[]>(
    initialData?.channelTypes ?? [],
  );
  const [schemas, setSchemas] = useState(initialData?.schemas);
  const [loading, setLoading] = useState(initialData === undefined);
  const hasLoaded = useRef(initialData !== undefined);

  const fetchChannels = useCallback(async () => {
    if (!hasLoaded.current) setLoading(true);
    try {
      const [data, types, definitions] = await Promise.all([
        api.listChannels(),
        api.listChannelTypes(),
        api.listChannelSchemas(),
      ]);
      if (data)
        setChannels(data as unknown as Record<string, Record<string, unknown>>);
      if (types) setChannelTypes(types);
      if (definitions) setSchemas(definitions);
      hasLoaded.current = true;
    } catch (error) {
      console.error("❌ Failed to load channels:", error);
    } finally {
      setLoading(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedAgent]);

  useEffect(() => {
    if (initialData) {
      setChannels(initialData.channels);
      setChannelTypes(initialData.channelTypes);
      setSchemas(initialData.schemas);
      hasLoaded.current = true;
      setLoading(false);
      return;
    }
    void fetchChannels();
  }, [fetchChannels, initialData]);

  const orderedKeys = useMemo(() => {
    const order = schemas?.order ?? ["console"];
    return [
      ...order.filter((key) => channelTypes.includes(key)),
      ...channelTypes.filter((key) => !order.includes(key)),
    ];
  }, [schemas, channelTypes]);

  const isBuiltin = useCallback(
    (key: string) => Boolean(channels[key]?.isBuiltin),
    [channels],
  );

  return {
    channels,
    schemas,
    channelTypes,
    orderedKeys,
    isBuiltin,
    loading,
    fetchChannels,
  };
}
