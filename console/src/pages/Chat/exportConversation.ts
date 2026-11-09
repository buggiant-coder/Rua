import {
  conversationApi,
  type ConversationApiTarget,
} from "@/api/modules/conversation";
import { getDownloadFolderHint, saveTextFile } from "@/utils/saveFile";
import { formatMessagesAsMarkdown, groupIntoMessageBlocks } from "./utils";

export async function exportConversationAsMarkdown(
  target: ConversationApiTarget,
  chatId: string,
): Promise<{ filename: string; folderHint: string; saved: boolean }> {
  const history = await conversationApi.getHistory(target, chatId);
  const blocks = groupIntoMessageBlocks(history.messages || [], true, true);
  const selected = new Set(blocks.map((_, index) => index));
  const markdown = formatMessagesAsMarkdown(blocks, selected, chatId);
  const filename = `${chatId}.md`;
  const result = await saveTextFile(
    markdown,
    filename,
    "text/markdown;charset=utf-8",
  );
  return {
    filename,
    folderHint: result.usedPicker ? "" : getDownloadFolderHint(),
    saved: result.ok,
  };
}
