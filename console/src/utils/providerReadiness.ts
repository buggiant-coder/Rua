export interface ProviderReadinessInput {
  id: string;
  is_custom?: boolean;
  is_local?: boolean;
  require_api_key?: boolean;
  api_key?: string;
  base_url?: string;
  models?: unknown[];
  extra_models?: unknown[];
}

const LOCAL_PROVIDER_ID = "rua-local";

export function isRustLocalProvider(provider: ProviderReadinessInput): boolean {
  return Boolean(provider.is_local || provider.id === LOCAL_PROVIDER_ID);
}

export function isLocalOrLanScopedProvider(
  provider: ProviderReadinessInput,
): boolean {
  if (isRustLocalProvider(provider)) return true;

  const host = extractHost(provider.base_url);
  if (!host) return false;
  const normalized = host.toLowerCase();

  if (
    normalized === "localhost" ||
    normalized === "0.0.0.0" ||
    normalized === "::1" ||
    normalized.endsWith(".local")
  ) {
    return true;
  }

  if (normalized.startsWith("127.")) return true;
  if (normalized.startsWith("10.")) return true;
  if (normalized.startsWith("192.168.")) return true;
  if (normalized.startsWith("169.254.")) return true;

  const match = normalized.match(/^172\.(\d+)\./);
  if (match) {
    const secondOctet = Number(match[1]);
    return secondOctet >= 16 && secondOctet <= 31;
  }

  return normalized.startsWith("fc") || normalized.startsWith("fd");
}

export function providerHasModels(provider: ProviderReadinessInput): boolean {
  return (
    (provider.models?.length ?? 0) + (provider.extra_models?.length ?? 0) > 0
  );
}

export function isProviderConfigured(
  provider: ProviderReadinessInput,
): boolean {
  if (isRustLocalProvider(provider)) return true;
  if (provider.require_api_key === false) return true;
  if (!provider.api_key) return false;
  if (provider.is_custom) return !!provider.base_url;
  return true;
}

export function isProviderReadyForModelSelection(
  provider: ProviderReadinessInput,
): boolean {
  return isProviderConfigured(provider) && providerHasModels(provider);
}

function extractHost(baseUrl?: string): string | null {
  const raw = baseUrl?.trim();
  if (!raw) return null;

  try {
    const url = new URL(raw.includes("://") ? raw : `http://${raw}`);
    return url.hostname.replace(/^\[(.*)\]$/, "$1");
  } catch {
    return null;
  }
}
