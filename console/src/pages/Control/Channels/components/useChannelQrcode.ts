import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../../../api";

interface ChannelQrcodeConfig {
  channel: string;

  successStatus: string;

  successCredentialKey: string;

  pollInterval?: number;

  params?: Record<string, string>;

  onSuccess: (credentials: Record<string, string>) => void;

  onExpired?: () => void;

  onError: (type: "fetch" | "expired" | "fail") => void;
}

interface ChannelQrcodeState {
  qrcodeImg: string;
  loading: boolean;
  fetchQrcode: () => Promise<void>;
  stopPoll: () => void;
  reset: () => void;
}

export function useChannelQrcode(
  config: ChannelQrcodeConfig,
): ChannelQrcodeState {
  const {
    channel,
    successStatus,
    successCredentialKey,
    pollInterval = 2000,
    params,
    onSuccess,
    onExpired,
    onError,
  } = config;

  const [qrcodeImg, setQrcodeImg] = useState("");
  const [loading, setLoading] = useState(false);
  const pollRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const confirmedRef = useRef(false);

  const stopPoll = useCallback(() => {
    if (pollRef.current) {
      clearTimeout(pollRef.current);
      pollRef.current = null;
    }
  }, []);

  const reset = useCallback(() => {
    stopPoll();
    setQrcodeImg("");
    confirmedRef.current = false;
  }, [stopPoll]);

  const fetchQrcode = useCallback(async () => {
    reset();
    setLoading(true);
    try {
      const data = await api.getChannelQrcode(channel, params);
      if (!data.qrcode_img) {
        onError("fetch");
        return;
      }
      setQrcodeImg(data.qrcode_img);

      const schedulePoll = () => {
        pollRef.current = setTimeout(async () => {
          try {
            const result = await api.getChannelQrcodeStatus(
              channel,
              data.poll_token,
              params,
            );
            if (
              result.status === successStatus &&
              result.credentials[successCredentialKey]
            ) {
              if (confirmedRef.current) return;
              confirmedRef.current = true;
              setQrcodeImg("");
              onSuccess(result.credentials);
              return;
            } else if (result.status === "expired") {
              setQrcodeImg("");
              onExpired?.();
              onError("expired");
              return;
            } else if (result.status === "fail") {
              setQrcodeImg("");
              onError("fail");
              return;
            }
          } catch {}

          schedulePoll();
        }, pollInterval);
      };
      schedulePoll();
    } catch {
      onError("fetch");
    } finally {
      setLoading(false);
    }
  }, [
    channel,
    successStatus,
    successCredentialKey,
    pollInterval,
    params,
    onSuccess,
    onExpired,
    onError,
    reset,
    stopPoll,
  ]);

  useEffect(() => stopPoll, [stopPoll]);

  return { qrcodeImg, loading, fetchQrcode, stopPoll, reset };
}
