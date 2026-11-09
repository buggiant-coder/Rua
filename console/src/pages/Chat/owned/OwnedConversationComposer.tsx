import {
  memo,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";
import {
  ArrowUp,
  ChevronsDown,
  ChevronsUp,
  FileText,
  LoaderCircle,
  Paperclip,
  Square,
  X,
} from "lucide-react";
import { Btn, Tooltip, toast } from "@/ui";
import { chatApi, type ChatMediaScope } from "@/api/modules/chat";
import type { ModelSlot } from "@/api/types";
import ImageViewer from "@/components/ImageViewer";
import { useSlashCommands } from "@/hooks/useSlashCommands";
import { toDisplayUrl } from "../utils";
import * as drafts from "../drafts";
import { requestBrowserNotificationPermissionFromUserGesture } from "@/userNotifications/browserPermission";
import "./owned-conversation-composer.css";

const TEXTAREA_COLLAPSED_MAX_HEIGHT = 220;
const TEXTAREA_EXPANDED_MIN_HEIGHT = 164;
const TEXTAREA_EXPANDED_MAX_HEIGHT = 440;
const LONG_PASTE_CHARACTER_THRESHOLD = 50_000;

export type ComposerAttachment = {
  id: string;
  name: string;
  type: "image" | "video" | "audio" | "file";
  url: string;
  marker?: string;
  storedName?: string;
  mimeType?: string;
  size?: number;
  authorityEpoch?: string;
};

export type RecalledComposerDraft = {
  draftKey?: string;
  text: string;
  attachments: ComposerAttachment[];
};

type PendingComposerAttachment = {
  id: string;
  name: string;
  type: ComposerAttachment["type"];
  marker?: string;
  previewUrl?: string;
};

type OwnedConversationComposerProps = {
  chatId: string;
  mediaScope: ChatMediaScope;
  uploadAgentId?: string;
  chatModel?: ModelSlot | null;
  currentAuthorityEpoch?: string | null;
  disabled?: boolean;
  submitDisabled?: boolean;
  submitDisabledReason?: string;
  attachmentsDisabled?: boolean;
  turnActive: boolean;
  draftKey?: string;
  placeholder?: string;
  recalledDraft?: RecalledComposerDraft | null;
  onSubmit: (
    text: string,
    attachments?: ComposerAttachment[],
  ) => Promise<boolean | "unknown" | void> | boolean | "unknown" | void;
  onStop: () => Promise<void> | void;

  deviceChip?: React.ReactNode;

  leftTools?: ReactNode;
};

function attachmentKind(file: File): ComposerAttachment["type"] {
  if (file.type.startsWith("image/")) return "image";
  if (file.type.startsWith("video/")) return "video";
  if (file.type.startsWith("audio/")) return "audio";
  return "file";
}

function exceedsCharacterThreshold(text: string, threshold: number): boolean {
  let count = 0;
  for (const _character of text) {
    count += 1;
    if (count > threshold) return true;
  }
  return false;
}

function pastedTextFilename(now = new Date()): string {
  const timestamp = now.toISOString().replace(/[.:]/g, "-");
  return `pasted-text-${timestamp}.txt`;
}

function removeAttachmentMarker(text: string, marker?: string): string {
  if (!marker) return text;
  const index = text.indexOf(marker);
  if (index < 0) return text;
  let before = text.slice(0, index);
  let after = text.slice(index + marker.length);
  if (/\s$/.test(before) && /^\s/.test(after)) {
    after = after.slice(1);
  } else if (!after && /\s$/.test(before)) {
    before = before.slice(0, -1);
  } else if (!before && /^\s/.test(after)) {
    after = after.slice(1);
  }
  return `${before}${after}`;
}

function imageMarker(index: number): string {
  return `[Image #${index}]`;
}

function fileMarker(index: number): string {
  return `[File #${index}]`;
}

function insertAttachmentMarkers(
  text: string,
  markers: string[],
  textarea: HTMLTextAreaElement | null,
) {
  const start = textarea?.selectionStart ?? text.length;
  const end = textarea?.selectionEnd ?? start;
  const before = text.slice(0, start);
  const after = text.slice(end);
  const markerText = markers.join(" ");
  const prefix = before && !/\s$/.test(before) ? " " : "";
  const suffix = after && !/^\s/.test(after) ? " " : "";
  const insertion = `${prefix}${markerText}${suffix}`;
  return {
    next: `${before}${insertion}${after}`,
    cursor: before.length + insertion.length,
  };
}

function removeAndRelabelAttachment(
  text: string,
  attachments: ComposerAttachment[],
  removed: ComposerAttachment,
) {
  let nextText = removeAttachmentMarker(text, removed.marker);
  const remaining = attachments.filter((item) => item.id !== removed.id);
  const temporaryMarkers = new Map<string, string>();

  for (const attachment of remaining) {
    if (!attachment.marker) continue;
    const temporary = `\u0000rua-attachment-${attachment.id}\u0000`;
    temporaryMarkers.set(attachment.id, temporary);
    nextText = nextText.replace(attachment.marker, temporary);
  }

  let imageIndex = 0;
  let fileIndex = 0;
  const relabeled = remaining.map((attachment) => {
    if (!attachment.marker) return attachment;
    const marker =
      attachment.type === "image"
        ? imageMarker((imageIndex += 1))
        : fileMarker((fileIndex += 1));
    const temporary = temporaryMarkers.get(attachment.id);
    if (temporary) nextText = nextText.replace(temporary, marker);
    return { ...attachment, marker };
  });

  return { text: nextText, attachments: relabeled };
}

function attachmentIntersectingDeletion(
  text: string,
  attachments: ComposerAttachment[],
  selectionStart: number,
  selectionEnd: number,
  key: "Backspace" | "Delete",
) {
  return attachments.find((attachment) => {
    if (!attachment.marker) return false;
    const markerStart = text.indexOf(attachment.marker);
    if (markerStart < 0) return false;
    const markerEnd = markerStart + attachment.marker.length;

    if (selectionStart !== selectionEnd) {
      return selectionStart < markerEnd && selectionEnd > markerStart;
    }
    if (selectionStart > markerStart && selectionStart < markerEnd) return true;
    return key === "Backspace"
      ? selectionStart === markerEnd
      : selectionStart === markerStart;
  });
}

function splitTextByAttachmentMarkers(
  text: string,
  attachments: ComposerAttachment[],
): Array<string | ComposerAttachment> {
  const markedAttachments = attachments
    .filter((attachment) => attachment.marker)
    .sort(
      (left, right) => text.indexOf(left.marker!) - text.indexOf(right.marker!),
    );
  const parts: Array<string | ComposerAttachment> = [];
  let cursor = 0;

  for (const attachment of markedAttachments) {
    const markerStart = text.indexOf(attachment.marker!, cursor);
    if (markerStart < 0) continue;
    parts.push(text.slice(cursor, markerStart), attachment);
    cursor = markerStart + attachment.marker!.length;
  }
  parts.push(text.slice(cursor));
  return parts;
}

function localImagePreviewUrl(file: File): string | undefined {
  if (
    !file.type.startsWith("image/") ||
    typeof URL.createObjectURL !== "function"
  ) {
    return undefined;
  }
  return URL.createObjectURL(file);
}

function revokePreviewUrl(url?: string) {
  if (url && typeof URL.revokeObjectURL === "function") {
    URL.revokeObjectURL(url);
  }
}

export default memo(function OwnedConversationComposer({
  chatId,
  mediaScope,
  uploadAgentId,
  chatModel,
  currentAuthorityEpoch,
  disabled = false,
  submitDisabled = false,
  submitDisabledReason,
  attachmentsDisabled = false,
  turnActive,
  draftKey,
  placeholder = "Message",
  recalledDraft,
  onSubmit,
  onStop,
  deviceChip,
  leftTools,
}: OwnedConversationComposerProps) {
  const { t } = useTranslation();
  const tx = (key: string, fallback: string) => {
    const translated = t(key);
    return translated === key ? fallback : translated;
  };
  const initialDraftRef = useRef<drafts.ChatDraft | null>(null);
  if (initialDraftRef.current === null) {
    initialDraftRef.current =
      draftKey && drafts.loadSubmitted(draftKey)
        ? { text: "", attachments: [] }
        : draftKey
        ? drafts.loadDraft(draftKey)
        : { text: "", attachments: [] };
  }
  const [value, setValue] = useState(initialDraftRef.current.text);
  const [attachments, setAttachments] = useState<ComposerAttachment[]>(
    initialDraftRef.current.attachments,
  );
  const [pendingAttachments, setPendingAttachments] = useState<
    PendingComposerAttachment[]
  >([]);
  const attachmentSourcesRef = useRef(new Map<string, File>());
  const [uploading, setUploading] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [slashOpen, setSlashOpen] = useState(false);
  const [slashActiveIndex, setSlashActiveIndex] = useState(0);
  const [expanded, setExpanded] = useState(false);
  const [markerPreview, setMarkerPreview] = useState<ComposerAttachment | null>(
    null,
  );
  const composingRef = useRef(false);
  const [composing, setComposing] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const markerLayerRef = useRef<HTMLDivElement>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const valueRef = useRef(value);
  const showMarkerLayer =
    !composing &&
    attachments.some((item) => item.marker && value.includes(item.marker));
  const canSubmit =
    !uploading && !submitting && (!!value.trim() || attachments.length > 0);
  const uploadAttachment = async (file: File) => {
    if (uploadAgentId || chatModel) {
      return chatApi.uploadFile(mediaScope, chatId, file, {
        agentId: uploadAgentId,
        model: chatModel,
      });
    }
    return chatApi.uploadFile(mediaScope, chatId, file);
  };
  const attachmentDisplayUrl = (attachment: ComposerAttachment) =>
    attachment.storedName
      ? chatApi.mediaUrl(mediaScope, chatId, attachment.storedName)
      : toDisplayUrl(attachment.url);
  const showStopAction = turnActive && !canSubmit;
  const slashCommands = useSlashCommands();

  const slashQuery =
    value.startsWith("/") && !value.includes("\n")
      ? value.slice(1).toLowerCase()
      : "";
  const filteredSlashCommands = slashOpen
    ? slashCommands
        .filter((item) => item.command.toLowerCase().includes(`/${slashQuery}`))
        .slice(0, 8)
    : [];
  const activeSlashCommand =
    filteredSlashCommands[
      Math.min(slashActiveIndex, filteredSlashCommands.length - 1)
    ];

  useLayoutEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    if (
      typeof CSS !== "undefined" &&
      CSS.supports?.("field-sizing", "content")
    ) {
      textarea.style.removeProperty("height");
      return;
    }
    const minHeight = expanded ? TEXTAREA_EXPANDED_MIN_HEIGHT : 0;
    const maxHeight = expanded
      ? TEXTAREA_EXPANDED_MAX_HEIGHT
      : TEXTAREA_COLLAPSED_MAX_HEIGHT;
    const measurement = textarea.cloneNode(false) as HTMLTextAreaElement;
    measurement.removeAttribute("id");
    measurement.setAttribute("aria-hidden", "true");
    measurement.tabIndex = -1;
    measurement.value = textarea.value;
    Object.assign(measurement.style, {
      position: "absolute",
      visibility: "hidden",
      pointerEvents: "none",
      width: `${textarea.getBoundingClientRect().width}px`,
      height: "0px",
      minHeight: "0px",
      maxHeight: "none",
      overflow: "hidden",
    });
    textarea.parentElement!.appendChild(measurement);
    const height = `${Math.min(
      Math.max(measurement.scrollHeight, minHeight),
      maxHeight,
    )}px`;
    measurement.remove();
    if (textarea.style.height !== height) textarea.style.height = height;
  }, [expanded, value]);

  useLayoutEffect(() => {
    if (!draftKey) {
      valueRef.current = "";
      setValue("");
      setAttachments([]);
      return;
    }
    const draft = drafts.loadSubmitted(draftKey)
      ? { text: "", attachments: [] }
      : drafts.loadDraft(draftKey);
    valueRef.current = draft.text;
    setValue(draft.text);
    setAttachments(draft.attachments);
  }, [draftKey]);

  useEffect(() => {
    if (!draftKey) return;
    const flush = () => drafts.flush(draftKey);
    const onVisibilityChange = () => {
      if (document.hidden) flush();
    };
    window.addEventListener("pagehide", flush);
    window.addEventListener("rua:background", flush);
    document.addEventListener("visibilitychange", onVisibilityChange);
    return () => {
      flush();
      window.removeEventListener("pagehide", flush);
      window.removeEventListener("rua:background", flush);
      document.removeEventListener("visibilitychange", onVisibilityChange);
    };
  }, [draftKey]);

  useLayoutEffect(() => {
    const textarea = textareaRef.current;
    const layer = markerLayerRef.current;
    if (showMarkerLayer && textarea && layer) {
      layer.scrollTop = textarea.scrollTop;
      layer.scrollLeft = textarea.scrollLeft;
    }
  }, [showMarkerLayer]);

  useEffect(() => {
    setSlashActiveIndex(0);
  }, [slashOpen, slashQuery]);

  useEffect(() => {
    if (!recalledDraft || recalledDraft.draftKey !== draftKey) return;
    setValue(recalledDraft.text);
    setAttachments(recalledDraft.attachments);
    valueRef.current = recalledDraft.text;
    setSlashOpen(
      recalledDraft.text.startsWith("/") && !recalledDraft.text.includes(" "),
    );
    if (draftKey)
      drafts.save(draftKey, recalledDraft.text, recalledDraft.attachments);

    const focusTextarea = () => {
      const textarea = textareaRef.current;
      if (!textarea) return;
      textarea.focus();
      const cursor = recalledDraft.text.length;
      textarea.setSelectionRange(cursor, cursor);
    };
    if (typeof window.requestAnimationFrame === "function") {
      window.requestAnimationFrame(focusTextarea);
    } else {
      window.setTimeout(focusTextarea, 0);
    }
  }, [draftKey, recalledDraft]);

  const submit = async () => {
    const text = value.trim();
    if (
      (!text && attachments.length === 0) ||
      disabled ||
      submitDisabled ||
      submitting
    ) {
      return;
    }
    if (draftKey) drafts.flush(draftKey);
    requestBrowserNotificationPermissionFromUserGesture();
    let submittedAttachments = attachments;
    let inputTransferred = false;
    setSubmitting(true);
    try {
      if (
        currentAuthorityEpoch &&
        attachments.some(
          (attachment) =>
            attachment.authorityEpoch &&
            attachment.authorityEpoch !== currentAuthorityEpoch,
        )
      ) {
        const refreshed = await Promise.all(
          attachments.map(async (attachment) => {
            if (
              !attachment.authorityEpoch ||
              attachment.authorityEpoch === currentAuthorityEpoch
            ) {
              return attachment;
            }
            const source = attachmentSourcesRef.current.get(attachment.id);
            if (!source) {
              throw new Error(
                `Attachment '${attachment.name}' must be selected again after the Authority changed`,
              );
            }
            const response = await uploadAttachment(source);
            return {
              ...attachment,
              name: response.original_filename || source.name,
              url: response.media_ref,
              storedName: response.media_id,
              mimeType: response.mime_type,
              size: response.byte_size,
              authorityEpoch: response.authority_epoch,
            };
          }),
        );
        submittedAttachments = refreshed;
        setAttachments(refreshed);
        if (draftKey) drafts.save(draftKey, valueRef.current, refreshed);
      }
      inputTransferred = true;
      setValue("");
      setAttachments([]);
      const accepted = await onSubmit(text, submittedAttachments);
      if (accepted === "unknown") {
        valueRef.current = "";
        return;
      }
      if (accepted === false) {
        setValue(value);
        setAttachments(submittedAttachments);
        return;
      }
      setValue("");
      setAttachments([]);
      valueRef.current = "";
      setSlashOpen(false);
      if (draftKey) drafts.clear(draftKey);
    } catch (error) {
      if (inputTransferred) {
        setValue(value);
        setAttachments(submittedAttachments);
      }
      toast.error(
        error instanceof Error
          ? error.message
          : tx("chat.attachments.uploadFailed", "Attachment upload failed"),
      );
    } finally {
      setSubmitting(false);
    }
  };

  const updateValue = (next: string) => {
    valueRef.current = next;
    setValue(next);
    if (draftKey) drafts.scheduleSave(draftKey, next, attachments);
    setSlashOpen(next.startsWith("/") && !next.includes(" "));
  };

  const completeSlashCommand = (command: string) => {
    updateValue(`${command} `);
    setSlashOpen(false);
  };

  const uploadFiles = async (
    files: FileList | File[],
    markFiles = false,
  ): Promise<boolean> => {
    const selected = Array.from(files);
    if (selected.length === 0 || disabled || attachmentsDisabled || submitting)
      return false;
    let nextImageIndex =
      attachments.filter((attachment) => attachment.type === "image").length +
      pendingAttachments.filter((attachment) => attachment.type === "image")
        .length;
    let nextFileIndex =
      attachments.filter(
        (attachment) => attachment.type === "file" && attachment.marker,
      ).length +
      pendingAttachments.filter(
        (attachment) => attachment.type === "file" && attachment.marker,
      ).length;
    const pending = selected.map((file) => {
      const id = crypto.randomUUID();
      const type = attachmentKind(file);
      const marker =
        type === "image"
          ? imageMarker((nextImageIndex += 1))
          : markFiles && type === "file"
          ? fileMarker((nextFileIndex += 1))
          : undefined;
      return {
        file,
        id,
        type,
        name: file.name,
        marker,
        previewUrl: localImagePreviewUrl(file),
      };
    });
    const markers = pending.flatMap((item) =>
      item.marker ? [item.marker] : [],
    );
    if (markers.length > 0) {
      const inserted = insertAttachmentMarkers(
        value,
        markers,
        textareaRef.current,
      );
      updateValue(inserted.next);
      window.setTimeout(() => {
        textareaRef.current?.focus();
        textareaRef.current?.setSelectionRange(
          inserted.cursor,
          inserted.cursor,
        );
      }, 0);
    }
    setPendingAttachments((current) => [...current, ...pending]);
    setUploading(true);
    try {
      const uploaded = await Promise.all(
        pending.map(async ({ file, id, marker, type }) => {
          const response = await uploadAttachment(file);
          attachmentSourcesRef.current.set(id, file);
          return {
            id,
            name: response.original_filename || file.name,
            type,
            url: response.media_ref,
            marker,
            storedName: response.media_id,
            mimeType: response.mime_type,
            size: response.byte_size,
            authorityEpoch: response.authority_epoch,
          } satisfies ComposerAttachment;
        }),
      );
      setAttachments((prev) => {
        const next = [...prev, ...uploaded];
        if (draftKey) drafts.save(draftKey, valueRef.current, next);
        return next;
      });
      return true;
    } catch (err) {
      setValue((current) => {
        const next = pending.reduce(
          (result, item) => removeAttachmentMarker(result, item.marker),
          current,
        );
        valueRef.current = next;
        if (draftKey) drafts.save(draftKey, next, attachments);
        return next;
      });
      toast.error(err instanceof Error ? err.message : String(err));
      return false;
    } finally {
      const pendingIds = new Set<string>(pending.map((item) => item.id));
      setPendingAttachments((current) =>
        current.filter((item) => !pendingIds.has(item.id)),
      );
      pending.forEach((item) => revokePreviewUrl(item.previewUrl));
      setUploading(false);
      if (fileInputRef.current) fileInputRef.current.value = "";
      textareaRef.current?.focus();
    }
  };

  const toolDisabled = disabled || attachmentsDisabled || submitting;

  return (
    <div
      className={
        expanded
          ? "rua-owned-composer rua-owned-composer--expanded"
          : "rua-owned-composer"
      }
    >
      {(pendingAttachments.length > 0 || attachments.length > 0) && (
        <div className="rua-owned-composer-attachments" aria-live="polite">
          {pendingAttachments.map((attachment) => (
            <div
              key={`pending-${attachment.id}`}
              className={[
                "rua-owned-composer-attachment",
                attachment.type === "image" ? "is-image" : "is-file",
                "is-uploading",
              ].join(" ")}
              title={attachment.name}
            >
              {attachment.type === "image" && attachment.previewUrl ? (
                <ImageViewer
                  src={attachment.previewUrl}
                  alt={attachment.name}
                  imageClassName="rua-owned-composer-attachment-image"
                />
              ) : (
                <>
                  <FileText size={16} aria-hidden="true" />
                  <span>{attachment.name}</span>
                </>
              )}
              <span className="rua-owned-composer-uploading">
                <LoaderCircle size={16} aria-hidden="true" />
              </span>
            </div>
          ))}
          {attachments.map((attachment) => (
            <div
              key={attachment.id}
              className={[
                "rua-owned-composer-attachment",
                attachment.type === "image" ? "is-image" : "is-file",
              ].join(" ")}
              title={attachment.name}
            >
              {attachment.type === "image" ? (
                <ImageViewer
                  src={attachmentDisplayUrl(attachment)}
                  alt={attachment.name}
                  imageClassName="rua-owned-composer-attachment-image"
                />
              ) : (
                <>
                  <FileText size={16} aria-hidden="true" />
                  <span>{attachment.name}</span>
                </>
              )}
              <button
                type="button"
                className="rua-owned-composer-attachment-remove"
                aria-label={`Remove ${attachment.name}`}
                onClick={() => {
                  const next = removeAndRelabelAttachment(
                    value,
                    attachments,
                    attachment,
                  );
                  setAttachments(next.attachments);
                  updateValue(next.text);
                }}
              >
                <X size={14} aria-hidden="true" />
              </button>
            </div>
          ))}
        </div>
      )}
      <div className="rua-owned-composer-editor">
        {showMarkerLayer && (
          <div
            ref={markerLayerRef}
            className="rua-owned-composer-marker-layer"
          >
            {splitTextByAttachmentMarkers(value, attachments).map(
              (part, index) =>
                typeof part === "string" ? (
                  part
                ) : part.type !== "image" ? (
                  <span
                    key={`${part.id}-${index}`}
                    className="rua-owned-composer-file-marker"
                  >
                    {part.marker}
                  </span>
                ) : (
                  <button
                    key={`${part.id}-${index}`}
                    type="button"
                    tabIndex={-1}
                    aria-label={`Open ${part.marker}`}
                    onClick={() => setMarkerPreview(part)}
                  >
                    {part.marker}
                  </button>
                ),
            )}
          </div>
        )}
        <textarea
          ref={textareaRef}
          className={showMarkerLayer ? "has-marker-layer" : undefined}
          value={value}
          placeholder={placeholder}
          disabled={disabled}
          readOnly={submitting}
          rows={1}
          onChange={(event) => updateValue(event.target.value)}
          onBlur={() => {
            if (draftKey) drafts.flush(draftKey);
          }}
          onScroll={(event) => {
            if (markerLayerRef.current) {
              markerLayerRef.current.scrollTop = event.currentTarget.scrollTop;
              markerLayerRef.current.scrollLeft =
                event.currentTarget.scrollLeft;
            }
          }}
          onPaste={(event) => {
            const files = event.clipboardData.files;
            if (files.length > 0) {
              event.preventDefault();
              void uploadFiles(files);
              return;
            }

            if (disabled || attachmentsDisabled || submitting) return;
            const pastedText = event.clipboardData.getData("text/plain");
            if (
              !exceedsCharacterThreshold(
                pastedText,
                LONG_PASTE_CHARACTER_THRESHOLD,
              )
            )
              return;

            event.preventDefault();
            const selectionStart = event.currentTarget.selectionStart;
            const selectionEnd = event.currentTarget.selectionEnd;
            const pastedFile = new File([pastedText], pastedTextFilename(), {
              type: "text/plain;charset=utf-8",
            });
            void uploadFiles([pastedFile], true).then((uploaded) => {
              if (uploaded) return;
              const current = valueRef.current;
              const start = Math.min(selectionStart, current.length);
              const end = Math.min(
                Math.max(selectionEnd, start),
                current.length,
              );
              const restored = `${current.slice(
                0,
                start,
              )}${pastedText}${current.slice(end)}`;
              updateValue(restored);
              window.setTimeout(() => {
                const cursor = start + pastedText.length;
                textareaRef.current?.focus();
                textareaRef.current?.setSelectionRange(cursor, cursor);
              }, 0);
            });
          }}
          onCompositionStart={() => {
            composingRef.current = true;
            setComposing(true);
          }}
          onCompositionEnd={() => {
            setComposing(false);
            window.setTimeout(() => {
              composingRef.current = false;
            }, 0);
          }}
          onKeyDown={(event) => {
            // WebKit can end composition before the confirming Enter keydown.
            if (
              composingRef.current ||
              event.nativeEvent.isComposing ||
              event.nativeEvent.keyCode === 229
            )
              return;
            if (event.key === "Backspace" || event.key === "Delete") {
              const attachment = attachmentIntersectingDeletion(
                value,
                attachments,
                event.currentTarget.selectionStart,
                event.currentTarget.selectionEnd,
                event.key,
              );
              if (attachment) {
                event.preventDefault();
                const next = removeAndRelabelAttachment(
                  value,
                  attachments,
                  attachment,
                );
                setAttachments(next.attachments);
                valueRef.current = next.text;
                setValue(next.text);
                if (draftKey)
                  drafts.save(draftKey, next.text, next.attachments);
                setSlashOpen(
                  next.text.startsWith("/") && !next.text.includes(" "),
                );
                return;
              }
            }
            if (slashOpen && filteredSlashCommands.length > 0) {
              if (event.key === "ArrowDown") {
                event.preventDefault();
                setSlashActiveIndex(
                  (prev) => (prev + 1) % filteredSlashCommands.length,
                );
                return;
              }
              if (event.key === "ArrowUp") {
                event.preventDefault();
                setSlashActiveIndex(
                  (prev) =>
                    (prev - 1 + filteredSlashCommands.length) %
                    filteredSlashCommands.length,
                );
                return;
              }
              if (event.key === "Escape") {
                event.preventDefault();
                setSlashOpen(false);
                return;
              }
              if (event.key === "Tab" || event.key === "Enter") {
                event.preventDefault();
                completeSlashCommand(activeSlashCommand.command);
                return;
              }
            }
            if (event.key === "Enter" && !event.shiftKey) {
              event.preventDefault();
              void submit();
            }
          }}
        />
      </div>
      {markerPreview && (
        <ImageViewer
          src={attachmentDisplayUrl(markerPreview)}
          alt={markerPreview.name}
          open
          hideTrigger
          onOpenChange={(open) => {
            if (!open) setMarkerPreview(null);
          }}
        />
      )}
      {filteredSlashCommands.length > 0 && (
        <div className="rua-owned-composer-slash" data-rua-chat-slash>
          {filteredSlashCommands.map((item, index) => (
            <button
              key={item.command}
              type="button"
              className={
                index === slashActiveIndex
                  ? "rua-owned-composer-slash-item is-active"
                  : "rua-owned-composer-slash-item"
              }
              aria-selected={index === slashActiveIndex}
              onMouseEnter={() => setSlashActiveIndex(index)}
              onMouseDown={(event) => {
                event.preventDefault();
                completeSlashCommand(item.command);
                textareaRef.current?.focus();
              }}
            >
              <span>{item.command}</span>
              <small>{item.description}</small>
            </button>
          ))}
        </div>
      )}
      <input
        ref={fileInputRef}
        type="file"
        multiple
        hidden
        onChange={(event) => {
          if (event.target.files) void uploadFiles(event.target.files);
        }}
      />
      <div className="rua-owned-composer-bottom">
        <div className="rua-owned-composer-prefix">
          {leftTools}
          <Tooltip title={tx("chat.attachFiles", "Attach files")}>
            <Btn
              variant="outline"
              icon={<Paperclip />}
              aria-label={tx("chat.attachFiles", "Attach files")}
              loading={uploading}
              disabled={toolDisabled}
              onClick={() => fileInputRef.current?.click()}
            />
          </Tooltip>
          <Tooltip
            title={
              expanded
                ? tx("chat.collapseInput", "Collapse input")
                : tx("chat.expandInput", "Expand input")
            }
          >
            <Btn
              variant="outline"
              icon={expanded ? <ChevronsDown /> : <ChevronsUp />}
              aria-label={
                expanded
                  ? tx("chat.collapseInput", "Collapse input")
                  : tx("chat.expandInput", "Expand input")
              }
              id="rua-input-expand-btn"
              onClick={() => {
                setExpanded((prev) => !prev);
                textareaRef.current?.focus();
              }}
            />
          </Tooltip>
        </div>
        <div className="rua-owned-composer-actions">
          {deviceChip}
          <Btn
            icon={
              showStopAction ? (
                <Square size={14} fill="currentColor" />
              ) : (
                <ArrowUp />
              )
            }
            aria-label={
              showStopAction
                ? tx("chat.stopGenerating", "Stop generating")
                : tx("chat.sendMessage", "Send message")
            }
            title={!showStopAction ? submitDisabledReason : undefined}
            onClick={showStopAction ? () => void onStop() : () => void submit()}
            disabled={
              disabled ||
              (!showStopAction && (submitDisabled || submitting || !canSubmit))
            }
          />
        </div>
      </div>
    </div>
  );
});
