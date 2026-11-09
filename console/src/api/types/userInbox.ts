export interface InboxSender {
  kind: string;
  id?: string;
  display_name: string;
}

interface InboxContent {
  sender: InboxSender;
  title: string;
  body_markdown: string;
}

export interface InboxMessage extends InboxContent {
  id: string;
  created_at: string;
  read_at?: string;
}

export interface InboxPage {
  items: InboxMessage[];
  next_cursor?: string;
  unread_count: number;
}

export interface InboxListOptions {
  cursor?: string;
  limit?: number;
  read?: boolean;
}
