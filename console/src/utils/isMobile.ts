let cached: boolean | null = null;

function compute(): boolean {
  if (typeof navigator === "undefined") return false;
  if (typeof window !== "undefined") {
    try {
      const sp = new URLSearchParams(window.location.search);
      if (sp.get("_mobile") === "1") return true;
    } catch {}
  }
  if (typeof document !== "undefined") {
    if (document.documentElement.classList.contains("mobile-mode")) return true;
  }
  return navigator.userAgent.includes("Mobile/");
}

export function isMobile(): boolean {
  if (cached === null) {
    cached = compute();

    if (cached && typeof document !== "undefined") {
      document.documentElement.classList.add("mobile-mode");
    }
  }
  return cached;
}

export function __resetMobileDetectionForTests(): void {
  if (import.meta.env.MODE !== "test") return;
  cached = null;
}
