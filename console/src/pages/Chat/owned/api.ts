const SELECTED_VARIANT_PREFIX = "rua_selected_variant_";

export type SelectedAssistantVariant = {
  turn_id: string;
  selected_variant_id: string;
};

function selectedVariantStorageKey(conversationId: string): string {
  return `${SELECTED_VARIANT_PREFIX}${conversationId}`;
}

export function getPendingSelectedVariant(
  conversationId: string | undefined,
): SelectedAssistantVariant | null {
  if (!conversationId) return null;
  try {
    const raw = sessionStorage.getItem(
      selectedVariantStorageKey(conversationId),
    );
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<SelectedAssistantVariant>;
    if (
      typeof parsed.turn_id === "string" &&
      typeof parsed.selected_variant_id === "string"
    ) {
      return {
        turn_id: parsed.turn_id,
        selected_variant_id: parsed.selected_variant_id,
      };
    }
  } catch {}
  return null;
}

export function setPendingSelectedVariant(
  conversationId: string,
  selected: SelectedAssistantVariant,
): void {
  try {
    sessionStorage.setItem(
      selectedVariantStorageKey(conversationId),
      JSON.stringify(selected),
    );
  } catch {}
}

export function clearPendingSelectedVariant(conversationId: string): void {
  try {
    sessionStorage.removeItem(selectedVariantStorageKey(conversationId));
  } catch {}
}
