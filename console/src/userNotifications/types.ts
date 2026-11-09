export interface UserNotificationSender {
  id: string;
  name: string;
}

export interface UserNotificationRequest {
  target_device_id: string;
  sender: UserNotificationSender;
  title: string;
  body?: string;
  rua_uri?: string;
}

export interface AndroidUserNotificationBridge {
  capability(): string;
  showUserNotification(payload: string): boolean;
}

declare global {
  interface Window {
    RuaNotificationNative?: AndroidUserNotificationBridge;
  }
}

export {};
