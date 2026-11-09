import React from "react";

export function parseErrorDetail(error: unknown): Record<string, any> | null {
  if (!(error instanceof Error)) return null;
  const idx = error.message.indexOf(" - ");
  if (idx === -1) return null;
  try {
    const parsed = JSON.parse(error.message.slice(idx + 3));
    return parsed?.detail || parsed;
  } catch {
    return null;
  }
}

export function extractFriendlyMessage(error: unknown): string {
  if (error == null) return "";
  const raw = error instanceof Error ? error.message : String(error);

  const idx = raw.indexOf(" - ");
  const head = idx === -1 ? raw : raw.slice(0, idx);

  const firstLine = head
    .split(/\r?\n/)
    .map((s) => s.trim())
    .find(Boolean);
  return (firstLine ?? head).slice(0, 240);
}

export function extractRawDetail(error: unknown): string {
  if (error == null) return "";
  const raw = error instanceof Error ? error.message : String(error);
  const idx = raw.indexOf(" - ");
  if (idx === -1) {
    if (error instanceof Error && error.stack && error.stack !== raw) {
      return error.stack;
    }
    return "";
  }
  const tail = raw.slice(idx + 3).trim();

  try {
    const parsed = JSON.parse(tail);
    return JSON.stringify(parsed, null, 2);
  } catch {
    return tail;
  }
}

interface ShowErrorModalOptions {
  title?: string;

  message?: string;

  error?: unknown;

  detail?: string;

  detailLabel?: string;
}

export function showErrorModal(
  modal: { error: (config: any) => any; confirm?: (config: any) => any },
  options: ShowErrorModalOptions,
): void {
  const friendly =
    options.message ?? extractFriendlyMessage(options.error) ?? "";
  const rawDetail = options.detail ?? extractRawDetail(options.error);
  const title = options.title ?? (friendly || "Error");

  const children: React.ReactNode[] = [];
  if (friendly) {
    children.push(
      React.createElement(
        "div",
        {
          key: "msg",
          style: { wordBreak: "break-word", whiteSpace: "pre-wrap" },
        },
        friendly,
      ),
    );
  }
  if (rawDetail) {
    children.push(
      React.createElement(
        "details",
        { key: "detail", style: { marginTop: 12 } },
        React.createElement(
          "summary",
          { style: { cursor: "pointer", color: "var(--text-muted, #888)" } },
          options.detailLabel ?? "Details",
        ),
        React.createElement(
          "pre",
          {
            style: {
              marginTop: 8,
              maxHeight: 240,
              overflow: "auto",
              fontSize: 12,
              lineHeight: 1.5,
              whiteSpace: "pre-wrap",
              wordBreak: "break-word",
              background: "var(--card-hover, rgba(0,0,0,0.04))",
              padding: 8,
              borderRadius: 0,
            },
          },
          rawDetail,
        ),
      ),
    );
  }

  modal.error({
    title,
    width: 480,
    centered: true,
    content: React.createElement(
      "div",
      { style: { maxWidth: "100%" } },
      ...children,
    ),
  });
}
