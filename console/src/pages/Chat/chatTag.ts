import type { ChatTag } from "@/api/types/chat";

const CHAT_TAG_SEQUENCE: ChatTag[] = ["none", "red", "yellow", "green"];

export function nextChatTag(tag: ChatTag | undefined): ChatTag {
  const currentIndex = CHAT_TAG_SEQUENCE.indexOf(tag ?? "none");
  return CHAT_TAG_SEQUENCE[(currentIndex + 1) % CHAT_TAG_SEQUENCE.length];
}
