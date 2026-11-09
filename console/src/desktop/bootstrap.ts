import { invoke } from "@tauri-apps/api/core";

import "./bootstrap.css";
import {
  BootstrapController,
  type BootstrapState,
  type RuntimeStatus,
  type RuaRootSelection,
} from "./bootstrapController";

interface DesktopLabels {
  startingTitle: string;
  connecting: string;
  connectingElapsed: string;
  readyTitle: string;
  openingConsole: string;
  timeoutTitle: string;
  failedTitle: string;
  failedMessage: string;
  retry: string;
  chooseRuaRoot: string;
  runtimeTimeout: string;
}

interface DesktopBootstrapContext {
  locale: string;
  labels: DesktopLabels;
  ruaRoot: string;
  ruaRootEnvironmentManaged: boolean;
}

const defaultLabels: DesktopLabels = {
  startingTitle: "正在启动 Rua",
  connecting: "正在连接本地运行时...",
  connectingElapsed: "正在连接本地运行时，已等待 {seconds} 秒...",
  readyTitle: "Rua 已就绪",
  openingConsole: "正在打开控制台...",
  timeoutTitle: "启动超时",
  failedTitle: "启动失败",
  failedMessage: "本地运行时未能启动。",
  retry: "重试",
  chooseRuaRoot: "选择 Rua 根目录",
  runtimeTimeout: "Rua Runtime 未能在 {seconds} 秒内就绪。",
};

const statusTitle = element<HTMLHeadingElement>("statusTitle");
const statusMessage = element<HTMLParagraphElement>("statusMessage");
const spinner = element<HTMLDivElement>("spinner");
const failureIcon = element<HTMLDivElement>("failureIcon");
const errorDetails = element<HTMLPreElement>("errorDetails");
const failureActions = element<HTMLDivElement>("failureActions");
const retryButton = element<HTMLButtonElement>("retryButton");
const ruaRootButton = element<HTMLButtonElement>("ruaRootButton");

let labels = defaultLabels;
let ruaRootEnvironmentManaged = false;

const controller = new BootstrapController({
  getRuntimeStatus: () => invoke<RuntimeStatus>("desktop_runtime_status"),
  restartRuntime: () => invoke<void>("retry_desktop_runtime"),
  selectRuaRoot: () =>
    invoke<RuaRootSelection>("select_desktop_rua_root"),
  checkReadiness: fetchReadiness,
  navigate: (url) => window.location.replace(url),
  formatRuntimeTimeout: (seconds) =>
    interpolate(labels.runtimeTimeout, seconds),
});

controller.subscribe(render);
retryButton.addEventListener("click", () => {
  void controller.retry();
});
ruaRootButton.addEventListener("click", () => {
  void controller.changeRuaRoot();
});
window.addEventListener("beforeunload", () => controller.stop());

void initialize();

async function initialize(): Promise<void> {
  try {
    const context = await invoke<DesktopBootstrapContext>(
      "desktop_bootstrap_context",
    );
    labels = context.labels;
    ruaRootEnvironmentManaged = context.ruaRootEnvironmentManaged;
    document.documentElement.lang = context.locale;
    retryButton.textContent = labels.retry;
    ruaRootButton.textContent = labels.chooseRuaRoot;
    render(controller.state);
  } catch {}
  await controller.start();
}

function render(state: BootstrapState): void {
  const failed = state.phase === "error" || state.phase === "timeout";
  spinner.hidden = failed;
  failureIcon.hidden = !failed;
  failureActions.hidden = !failed;
  ruaRootButton.hidden = !failed || ruaRootEnvironmentManaged;
  errorDetails.hidden = !failed || !state.error;
  errorDetails.textContent = state.error;

  if (state.phase === "ready") {
    statusTitle.textContent = labels.readyTitle;
    statusMessage.textContent = labels.openingConsole;
    return;
  }
  if (failed) {
    statusTitle.textContent =
      state.phase === "timeout" ? labels.timeoutTitle : labels.failedTitle;
    statusMessage.textContent = labels.failedMessage;
    return;
  }

  statusTitle.textContent = labels.startingTitle;
  statusMessage.textContent =
    state.elapsedSeconds > 0
      ? interpolate(labels.connectingElapsed, state.elapsedSeconds)
      : labels.connecting;
}

function interpolate(template: string, seconds: number): string {
  return template.replace("{seconds}", String(seconds));
}

async function fetchReadiness(
  baseUrl: string,
  parentSignal: AbortSignal,
): Promise<boolean> {
  const request = new AbortController();
  const abort = () => request.abort();
  parentSignal.addEventListener("abort", abort, { once: true });
  const timeout = window.setTimeout(() => request.abort(), 1500);

  try {
    const response = await fetch(`${baseUrl}/readyz`, {
      cache: "no-store",
      signal: request.signal,
    });
    if (!response.ok) return false;
    const body = (await response.json()) as { ready?: boolean };
    return body.ready === true;
  } finally {
    window.clearTimeout(timeout);
    parentSignal.removeEventListener("abort", abort);
  }
}

function element<T extends HTMLElement>(id: string): T {
  const value = document.getElementById(id);
  if (!value) {
    throw new Error(`missing bootstrap element #${id}`);
  }
  return value as T;
}
