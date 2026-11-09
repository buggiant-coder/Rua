export function formatCompact(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "0";
  if (n >= 1e9) {
    const v = n / 1e9;
    return (
      (v % 1 === 0 ? v.toFixed(0) : v.toFixed(1).replace(/\.0$/, "")) + "B"
    );
  }
  if (n >= 1e6) {
    const v = n / 1e6;
    return (
      (v % 1 === 0 ? v.toFixed(0) : v.toFixed(1).replace(/\.0$/, "")) + "M"
    );
  }
  if (n >= 1e3) {
    const v = n / 1e3;
    return (
      (v % 1 === 0 ? v.toFixed(0) : v.toFixed(1).replace(/\.0$/, "")) + "K"
    );
  }
  return n.toLocaleString(undefined, { maximumFractionDigits: 0 });
}
