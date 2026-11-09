import type { Session } from "./constants";
import type { SortField, SortOrder } from "./FilterBar";

export const normalizeIsoTimestamp = (
  ts: string | null | undefined,
): string | null => {
  if (!ts) return null;
  if (/[Z+-]\d{2}:?\d{2}$/.test(ts) || ts.endsWith("Z")) return ts;
  return ts + "Z";
};

export const timestampValue = (ts: string | null | undefined): number => {
  const norm = normalizeIsoTimestamp(ts);
  if (!norm) return 0;
  const v = new Date(norm).getTime();
  return Number.isFinite(v) ? v : 0;
};

export function filterAndSortSessions(
  sessions: Session[],
  searchTitle: string,
  sortField: SortField,
  sortOrder: SortOrder,
): Session[] {
  const query = searchTitle.trim().toLowerCase();
  const filtered = query
    ? sessions.filter((session) =>
        (session.name || "").toLowerCase().includes(query),
      )
    : sessions;

  const collator = new Intl.Collator(undefined, {
    numeric: true,
    sensitivity: "base",
  });
  const dir = sortOrder === "asc" ? 1 : -1;

  return [...filtered].sort((a, b) => {
    switch (sortField) {
      case "name":
        return dir * collator.compare(a.name || "", b.name || "");
      case "user_id":
        return dir * collator.compare(a.user_id || "", b.user_id || "");
      case "channel":
        return dir * collator.compare(a.channel || "", b.channel || "");
      case "default":
      default:
        return (
          dir * (timestampValue(a.updated_at) - timestampValue(b.updated_at))
        );
    }
  });
}
