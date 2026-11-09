export const stripFrontmatter = (s: string): string =>
  s.replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n?/, "");
