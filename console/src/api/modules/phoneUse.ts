import { request } from "../request";

export interface PhoneUseStatus {
  generation: string;
  platform: "android";
  state: string;
  accessibility: boolean;
  camera_permission: boolean;
}

export const phoneUseApi = {
  status: () => request<PhoneUseStatus>("/platform/phone"),
};
