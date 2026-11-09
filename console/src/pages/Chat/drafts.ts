const INPUT_DRAFT_PREFIX = "rua_input_draft_";
const SUBMITTED_DRAFT_PREFIX = "rua_submitted_draft_";

export type SubmittedDraft = {
  submissionId: string;
  text: string;
  attachments: DraftAttachment[];
  request: Record<string, unknown>;
  targetDeviceId: string | null;
};

export function saveSubmitted(id: string, value: SubmittedDraft): void {
  const values = loadSubmittedAll(id).filter(
    (item) => item.submissionId !== value.submissionId,
  );
  sessionStorage.setItem(
    `${SUBMITTED_DRAFT_PREFIX}${id}`,
    JSON.stringify([...values, value]),
  );
}

export function loadSubmittedAll(id: string): SubmittedDraft[] {
  try {
    const values = JSON.parse(
      sessionStorage.getItem(`${SUBMITTED_DRAFT_PREFIX}${id}`) || "[]",
    );
    return Array.isArray(values)
      ? values.filter(
          (value) =>
            value &&
            typeof value.submissionId === "string" &&
            typeof value.text === "string" &&
            Array.isArray(value.attachments) &&
            value.request &&
            typeof value.request === "object",
        )
      : [];
  } catch {
    return [];
  }
}

export function loadSubmitted(
  id: string,
  submissionId?: string,
): SubmittedDraft | null {
  return (
    loadSubmittedAll(id).find(
      (item) => !submissionId || item.submissionId === submissionId,
    ) ?? null
  );
}

export function clearSubmitted(id: string, submissionId?: string): void {
  const remaining = submissionId
    ? loadSubmittedAll(id).filter((item) => item.submissionId !== submissionId)
    : [];
  if (remaining.length)
    sessionStorage.setItem(
      `${SUBMITTED_DRAFT_PREFIX}${id}`,
      JSON.stringify(remaining),
    );
  else sessionStorage.removeItem(`${SUBMITTED_DRAFT_PREFIX}${id}`);
}

export type DraftAttachment = {
  id: string;
  name: string;
  type: "image" | "video" | "audio" | "file";
  url: string;
  marker?: string;
  storedName?: string;
  mimeType?: string;
  size?: number;
};

export type ChatDraft = {
  text: string;
  attachments: DraftAttachment[];
};

export const DRAFT_KEY = (id: string): string => `${INPUT_DRAFT_PREFIX}${id}`;

const DRAFT_SAVE_INTERVAL_MS = 250;
const pendingSaves = new Map<
  string,
  { draft: ChatDraft; timer: ReturnType<typeof setTimeout> }
>();

function cancelPendingSave(id: string): void {
  const pending = pendingSaves.get(id);
  if (!pending) return;
  clearTimeout(pending.timer);
  pendingSaves.delete(id);
}

// Keep the latest text available immediately, but write at most once per
// interval. Updating an existing pending draft must not postpone it forever.
export function scheduleSave(
  id: string,
  text: string,
  attachments: DraftAttachment[] = [],
): void {
  const draft = { text, attachments };
  const pending = pendingSaves.get(id);
  if (pending) {
    pending.draft = draft;
    return;
  }
  pendingSaves.set(id, {
    draft,
    timer: setTimeout(() => flush(id), DRAFT_SAVE_INTERVAL_MS),
  });
}

export function flush(id: string): void {
  const pending = pendingSaves.get(id);
  if (pending) save(id, pending.draft.text, pending.draft.attachments);
}

export function save(
  id: string,
  text: string,
  attachments: DraftAttachment[] = [],
): void {
  cancelPendingSave(id);
  try {
    if (attachments.length > 0) {
      sessionStorage.setItem(
        DRAFT_KEY(id),
        JSON.stringify({ text, attachments } satisfies ChatDraft),
      );
    } else if (text) {
      sessionStorage.setItem(DRAFT_KEY(id), text);
    } else {
      sessionStorage.removeItem(DRAFT_KEY(id));
    }
  } catch {}
}

export function load(id: string): string {
  return loadDraft(id).text;
}

export function loadDraft(id: string): ChatDraft {
  const pending = pendingSaves.get(id);
  if (pending) return pending.draft;
  try {
    const stored = sessionStorage.getItem(DRAFT_KEY(id));
    if (!stored) return { text: "", attachments: [] };
    if (!stored.startsWith("{")) return { text: stored, attachments: [] };
    let parsed: Partial<ChatDraft>;
    try {
      parsed = JSON.parse(stored) as Partial<ChatDraft>;
    } catch {
      return { text: stored, attachments: [] };
    }
    return {
      text: typeof parsed.text === "string" ? parsed.text : "",
      attachments: Array.isArray(parsed.attachments)
        ? parsed.attachments.filter(isDraftAttachment)
        : [],
    };
  } catch {
    return { text: "", attachments: [] };
  }
}

function isDraftAttachment(value: unknown): value is DraftAttachment {
  if (!value || typeof value !== "object") return false;
  const attachment = value as Partial<DraftAttachment>;
  return (
    typeof attachment.id === "string" &&
    typeof attachment.name === "string" &&
    ["image", "video", "audio", "file"].includes(attachment.type || "") &&
    typeof attachment.url === "string" &&
    attachment.url.length > 0 &&
    !attachment.url.startsWith("blob:")
  );
}

export function clear(id: string): void {
  cancelPendingSave(id);
  try {
    sessionStorage.removeItem(DRAFT_KEY(id));
  } catch {}
}

// Project deletion must discard pending writes as well as stored drafts.
export function clearByPrefix(prefix: string): void {
  for (const id of pendingSaves.keys()) {
    if (id.startsWith(prefix)) clear(id);
  }
  try {
    const keys = Object.keys(sessionStorage);
    for (const key of keys) {
      if (key.startsWith(DRAFT_KEY(prefix))) sessionStorage.removeItem(key);
    }
  } catch {
    // Pending drafts are already discarded even if browser storage is unavailable.
  }
}
