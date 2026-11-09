import { useState, useEffect } from "react";

import { useTranslation } from "react-i18next";
import { syncApi, type SyncErrorReason } from "../../api/modules/sync";
import styles from "./index.module.less";
import { RefreshCcw } from "lucide-react";
import { Btn, Tooltip, toast } from "@/ui";

export default function SyncNowButton() {
  const { t } = useTranslation();
  const [loading, setLoading] = useState(false);
  const [enabled, setEnabled] = useState<boolean | null>(null);

  useEffect(() => {
    syncApi
      .getStatus()
      .then((s) => setEnabled(s.enabled))
      .catch(() => setEnabled(false));
  }, []);

  const reasonToast: Record<SyncErrorReason, string> = {
    not_paired: t("sync.notPaired", "未配对设备"),
    no_eligible_peer: t("sync.noEligiblePeer", "当前没有可连接的设备"),
    peer_unreachable: t("sync.peerUnreachable", "对端设备不可达"),
    signature_failed: t("sync.signatureFailed", "签名验证失败"),
    decrypt_failed: t("sync.decryptFailed", "解密失败"),
    timeout: t("sync.timeout", "同步超时"),
    repair_scan_failed: t("sync.repairScanFailed", "本机完整检查失败"),
    inbox_drain_failed: t("sync.inboxDrainFailed", "本机应用同步数据失败"),
  };

  const handleClick = async () => {
    setLoading(true);
    try {
      const res = await syncApi.triggerSyncCycle();
      const peer = res.peer_node_id.slice(0, 8);
      toast.success(
        t(
          "sync.successCurrentDevice",
          "本设备已补全（对端 {{peer}}），收到 {{received}} 条，发送 {{sent}} 条",
          {
            peer,
            received: res.events_received,
            sent: res.events_sent,
          },
        ),
      );
    } catch (err: unknown) {
      const reason =
        (err as { reason?: SyncErrorReason }).reason || "peer_unreachable";
      toast.error(reasonToast[reason] || reason);
    } finally {
      setLoading(false);
    }
  };

  if (!enabled) return null;

  return (
    <Tooltip title={t("sync.syncNow", "立即同步")} placement="top">
      <Btn
        variant="ghost"
        icon={<RefreshCcw size={14} className={loading ? "icon-spin" : ""} />}
        loading={loading}
        onClick={handleClick}
        className={styles.syncBtn}
        size="sm"
      />
    </Tooltip>
  );
}
