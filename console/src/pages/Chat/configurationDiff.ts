export interface ConfigurationChange {
  category: string;
  field: string;
  before: unknown;
  after: unknown;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function configurationChanges(
  before: Record<string, unknown>,
  after: Record<string, unknown>,
): ConfigurationChange[] {
  const changes: ConfigurationChange[] = [];
  function visit(left: unknown, right: unknown, path: string[]) {
    if (Object.is(left, right)) return;
    if (isRecord(left) && isRecord(right)) {
      for (const key of [
        ...new Set([...Object.keys(left), ...Object.keys(right)]),
      ].sort()) {
        visit(left[key], right[key], [...path, key]);
      }
      return;
    }
    if (
      Array.isArray(left) &&
      Array.isArray(right) &&
      JSON.stringify(left) === JSON.stringify(right)
    )
      return;
    changes.push({
      category: path[0],
      field: path.join("."),
      before: left,
      after: right,
    });
  }
  visit(before, after, []);
  return changes;
}
