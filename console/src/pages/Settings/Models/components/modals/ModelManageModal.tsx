import type { ProviderInfo } from "../../../../../api/types";
import { LocalModelManageModal } from "./LocalModelManageModal";
import { RemoteModelManageModal } from "./RemoteModelManageModal";

interface ModelManageModalProps {
  provider: ProviderInfo;
  open: boolean;
  onClose: () => void;
  onSaved: () => void;
}

export function ModelManageModal({
  provider,
  open,
  onClose,
  onSaved,
}: ModelManageModalProps) {
  if (provider.id === "rua-local") {
    return (
      <LocalModelManageModal
        provider={provider}
        open={open}
        onClose={onClose}
        onSaved={onSaved}
      />
    );
  }

  return (
    <RemoteModelManageModal
      provider={provider}
      open={open}
      onClose={onClose}
      onSaved={onSaved}
    />
  );
}
