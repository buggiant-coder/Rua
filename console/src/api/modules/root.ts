import { request } from "../request";

export const rootApi = {
  readRoot: () => request<unknown>("/"),
  getVersion: () => request<{ version: string }>("/version"),
};
