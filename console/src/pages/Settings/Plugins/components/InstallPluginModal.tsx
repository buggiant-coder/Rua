import { useState } from "react";
import { Btn, Check, Modal, toast } from "@/ui";
import { useTranslation } from "react-i18next";
import { pluginApi } from "../../../../api/modules/plugin";
import { InputWithExample } from "../../../../components/InputWithExample";

interface InstallPluginModalProps {
  open: boolean;
  onClose: () => void;
  onSuccess: () => void;
}

export default function InstallPluginModal({
  open,
  onClose,
  onSuccess,
}: InstallPluginModalProps) {
  const { t } = useTranslation();
  const [source, setSource] = useState("");
  const [force, setForce] = useState(false);
  const [loading, setLoading] = useState(false);

  const handleInstall = async () => {
    if (!source.trim()) return;
    setLoading(true);
    try {
      const result = await pluginApi.installPlugin(source.trim(), force);
      toast.success(t("plugins.installSuccess", { name: result.plugin_name }));
      setSource("");
      setForce(false);
      onClose();
      onSuccess();
    } catch (err) {
      const msg =
        err instanceof Error ? err.message : t("plugins.installFailed");
      toast.error(msg);
    } finally {
      setLoading(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={t("plugins.installPlugin")}
      destroyOnClose
      footer={
        <>
          <Btn variant="outline" onClick={onClose}>
            {t("common.cancel", "Cancel")}
          </Btn>
          <Btn
            onClick={handleInstall}
            loading={loading}
            disabled={!source.trim()}
          >
            {t("plugins.install")}
          </Btn>
        </>
      }
    >
      <div style={{ marginBottom: 16 }}>
        <label style={{ display: "block", marginBottom: 8 }}>
          {t("plugins.installUrl")}
        </label>
        <InputWithExample
          placeholder={t("plugins.installUrlPlaceholder")}
          example="https://github.com/user/plugin.git"
          value={source}
          onChange={(e) => setSource(e.target.value)}
          onPressEnter={handleInstall}
        />
      </div>
      <Check checked={force} onChange={(e) => setForce(e.target.checked)}>
        {t("plugins.installForce")}
      </Check>
    </Modal>
  );
}
