const SEQUENCE_MESSAGE_RE =
  /^(\s*[A-Za-z0-9_.@-]+\s*(?:<<-->>|<<->>|-->>|->>|-->|->|--x|-x|--\)|-\))\s*[+-]?[A-Za-z0-9_.@-]+\s*:\s*)(.*)$/;
const MERMAID_ESCAPED_SEMICOLON_RE =
  /#(?:\d+|x[0-9a-f]+);|&(?:#\d+|#x[0-9a-f]+|[a-z][a-z0-9]+);|;/gi;

function escapeSequenceMessageSemicolons(message: string): string {
  return message.replace(MERMAID_ESCAPED_SEMICOLON_RE, (token) =>
    token === ";" ? "#59;" : token,
  );
}

export function repairMermaidSource(source: string): string {
  const lines = source.split(/\r?\n/);
  const directive = lines.find((line) => line.trim())?.trim() || "";
  if (!/^sequenceDiagram\b/i.test(directive)) return source;

  return lines
    .map((line) => {
      const match = SEQUENCE_MESSAGE_RE.exec(line);
      if (!match || !match[2].includes(";")) return line;
      return `${match[1]}${escapeSequenceMessageSemicolons(match[2])}`;
    })
    .join("\n");
}
