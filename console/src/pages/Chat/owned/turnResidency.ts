export function retainConversationTurns({
  keys,
  previous,
  visibleStart,
  visibleEnd,
  protectedKeys,
  high,
  low,
  buffer,
}: {
  keys: readonly string[];
  previous: ReadonlySet<string>;
  visibleStart: number;
  visibleEnd: number;
  protectedKeys: ReadonlySet<string>;
  high: number;
  low: number;
  buffer: number;
}): Set<string> {
  if (keys.length <= high) return new Set(keys);
  const available = new Set(keys);
  const retained = new Set([...previous].filter((key) => available.has(key)));
  const required = new Set(
    [...protectedKeys].filter((key) => available.has(key)),
  );
  const start = Math.max(0, visibleStart - buffer);
  const end = Math.min(keys.length - 1, visibleEnd + buffer);
  for (let index = start; index <= end; index++) required.add(keys[index]);
  for (const key of required) retained.add(key);
  if (retained.size <= high) return retained;
  const center = (visibleStart + visibleEnd) / 2;
  const candidates = keys
    .map((key, index) => ({ key, distance: Math.abs(index - center) }))
    .filter(({ key }) => retained.has(key) && !required.has(key))
    .sort((a, b) => b.distance - a.distance);
  for (const { key } of candidates) {
    if (retained.size <= low) break;
    retained.delete(key);
  }
  return retained;
}
