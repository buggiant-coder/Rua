import { expect, test, type BrowserContext, type Page } from "@playwright/test";

const controllerUrl = process.env.RUA_PROJECT_E2E_CONTROLLER_URL;
const secondViewerUrl = process.env.RUA_PROJECT_E2E_SECOND_VIEWER_URL;
const mockLlmUrl = process.env.RUA_PROJECT_E2E_MOCK_LLM_URL;
const projectId = process.env.RUA_PROJECT_E2E_PROJECT_ID;
const projectName = process.env.RUA_PROJECT_E2E_PROJECT_NAME;
const chatId = process.env.RUA_PROJECT_E2E_CHAT_ID;
const providerId = process.env.RUA_PROJECT_E2E_PROVIDER_ID;
const modelId = process.env.RUA_PROJECT_E2E_MODEL_ID;
const resourceDeviceId = process.env.RUA_PROJECT_E2E_RESOURCE_DEVICE_ID;
const replyMarker = process.env.RUA_PROJECT_E2E_REPLY_MARKER;
const submitDeadlineMs = Number(
  process.env.RUA_PROJECT_E2E_SUBMIT_DEADLINE_MS ?? "3500",
);

const configured = [
  controllerUrl,
  secondViewerUrl,
  mockLlmUrl,
  projectId,
  projectName,
  chatId,
  providerId,
  modelId,
  resourceDeviceId,
  replyMarker,
].every(Boolean);

test.skip(
  !configured,
  "requires the Project Chat independent three-server harness",
);

function projectChatPath(baseUrl: string): string {
  return `${baseUrl}/projects/${encodeURIComponent(
    projectId!,
  )}/chats/${encodeURIComponent(chatId!)}`;
}

function apiPath(suffix: string): string {
  return `/api/projects/${encodeURIComponent(
    projectId!,
  )}/chats/${encodeURIComponent(chatId!)}${suffix}`;
}

async function configureContext(
  context: BrowserContext,
  lazy: boolean,
): Promise<void> {
  await context.addInitScript(
    ({ project, chat, provider, model, isLazy }) => {
      localStorage.setItem("language", "en");
      localStorage.setItem(`rua_project_last_chat_${project}`, chat);
      localStorage.setItem("rua_project_last_project", project);
      if (!isLazy) return;
      sessionStorage.setItem(`rua_project_lazy_chat_${project}:${chat}`, "1");
      sessionStorage.setItem(
        `rua_project_chat_agent_${project}:${chat}`,
        "default",
      );
      sessionStorage.setItem(
        `rua_project_chat_model_${project}:${chat}`,
        JSON.stringify({
          provider_id: provider,
          model,
          thinking_strength: "off",
        }),
      );
    },
    {
      project: projectId!,
      chat: chatId!,
      provider: providerId!,
      model: modelId!,
      isLazy: lazy,
    },
  );
}

async function openProjectChat(
  page: Page,
  baseUrl: string,
  requests: string[],
): Promise<void> {
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith("/api/")) {
      requests.push(`${request.method()} ${url.pathname}`);
    }
  });
  await page.goto(projectChatPath(baseUrl), {
    waitUntil: "domcontentloaded",
  });
  await expect(page.getByText(projectName!, { exact: true })).toBeVisible();
  await expect(page.locator("textarea").last()).toBeEditable();
}

test("Project Chat uses canonical authority routes for two live viewers before Sync", async ({
  browser,
  request,
}, testInfo) => {
  const controllerContext = await browser.newContext();
  const secondViewerContext = await browser.newContext();
  await configureContext(controllerContext, true);
  await configureContext(secondViewerContext, false);

  const controller = await controllerContext.newPage();
  let secondViewer = await secondViewerContext.newPage();
  const controllerRequests: string[] = [];
  const secondViewerRequests: string[] = [];
  const userText = `hold-project-first-${chatId}`;

  try {
    await openProjectChat(controller, controllerUrl!, controllerRequests);
    const composer = controller.locator("textarea").last();
    await composer.fill(userText);

    const submitResponsePromise = controller.waitForResponse((response) => {
      const url = new URL(response.url());
      return (
        response.request().method() === "POST" &&
        url.pathname === apiPath("/console/submit")
      );
    });
    const submitStartedAt = Date.now();
    await controller
      .getByRole("button", { name: "Send message", exact: true })
      .click();
    const submitResponse = await submitResponsePromise;
    const submitDurationMs = Date.now() - submitStartedAt;
    expect(submitResponse.status()).toBe(200);
    expect(submitDurationMs).toBeLessThan(submitDeadlineMs);
    const submitBody = await submitResponse.json();
    expect(submitBody.accepted).toBe(true);
    expect(submitBody.control?.placement?.authority_device_id).toBe(
      resourceDeviceId,
    );
    expect(submitBody.control?.placement?.switchable).toBe(false);
    expect(submitBody.control?.allowed_controls?.switch_authority).toBe(false);
    await expect(
      controller.locator("p").filter({ hasText: userText }).first(),
    ).toBeVisible();
    await expect(
      controller.getByRole("button", {
        name: "Stop generating",
        exact: true,
      }),
    ).toBeVisible();

    const secondLiveRequest = secondViewer.waitForRequest((request) => {
      const url = new URL(request.url());
      return (
        request.method() === "POST" && url.pathname === apiPath("/console/live")
      );
    });
    await openProjectChat(secondViewer, secondViewerUrl!, secondViewerRequests);
    await secondLiveRequest;
    await expect(
      secondViewer.locator("p").filter({ hasText: userText }).first(),
    ).toBeVisible();
    await expect(
      secondViewer.getByRole("button", {
        name: "Stop generating",
        exact: true,
      }),
    ).toBeVisible();

    const historyRequest = `GET ${apiPath("")}`;
    const secondHistoryReadsBeforeReconnect = secondViewerRequests.filter(
      (entry) => entry === historyRequest,
    ).length;
    expect(secondHistoryReadsBeforeReconnect).toBeLessThanOrEqual(6);
    await secondViewer.reload({ waitUntil: "domcontentloaded" });
    await expect(
      secondViewer.locator("p").filter({ hasText: userText }).first(),
    ).toBeVisible();
    await expect(
      secondViewer.getByRole("button", {
        name: "Stop generating",
        exact: true,
      }),
    ).toBeVisible();
    await secondViewer.waitForTimeout(750);
    const secondHistoryReadsAfterReconnect = secondViewerRequests.filter(
      (entry) => entry === historyRequest,
    ).length;
    expect(
      secondHistoryReadsAfterReconnect - secondHistoryReadsBeforeReconnect,
    ).toBeLessThanOrEqual(3);

    await secondViewer.close();
    const secondHistoryReadsBeforeGapReconnect = secondViewerRequests.filter(
      (entry) => entry === historyRequest,
    ).length;
    const release = await request.post(`${mockLlmUrl}/release`);
    expect(release.ok()).toBe(true);
    await expect(
      controller.getByText(replyMarker!, { exact: false }),
    ).toBeVisible({ timeout: 30_000 });

    secondViewer = await secondViewerContext.newPage();
    await openProjectChat(secondViewer, secondViewerUrl!, secondViewerRequests);
    await expect(
      secondViewer.getByText(replyMarker!, { exact: false }),
    ).toBeVisible({ timeout: 30_000 });
    await secondViewer.waitForTimeout(750);
    const secondHistoryReadsAfterGapReconnect = secondViewerRequests.filter(
      (entry) => entry === historyRequest,
    ).length;
    expect(
      secondHistoryReadsAfterGapReconnect -
        secondHistoryReadsBeforeGapReconnect,
    ).toBeLessThanOrEqual(6);
    await expect(
      controller
        .getByLabel("Pending messages")
        .getByText(userText, { exact: true }),
    ).toHaveCount(0);

    await expect(
      controller.getByRole("button", {
        name: "Stop generating",
        exact: true,
      }),
    ).toHaveCount(0, { timeout: 30_000 });

    const switchResponsePromise = controller.waitForResponse((response) => {
      const url = new URL(response.url());
      return (
        response.request().method() === "PATCH" &&
        url.pathname === apiPath("/agent-selection")
      );
    });
    await controller
      .getByRole("button", { name: "Project Chat Agent", exact: true })
      .click();
    await controller.getByRole("button", { name: /Code Agent/ }).click();
    await controller
      .getByRole("dialog")
      .getByRole("button", { name: "Switch Agent", exact: true })
      .click();
    expect((await switchResponsePromise).status()).toBe(200);

    await expect(
      controller.getByRole("button", {
        name: "Project Chat Agent",
        exact: true,
      }),
    ).toContainText("Code Agent");
    await expect(
      secondViewer.getByRole("button", {
        name: "Project Chat Agent",
        exact: true,
      }),
    ).toContainText("Code Agent");

    const secondHistoryReadsBeforeStableReload = secondViewerRequests.filter(
      (entry) => entry === historyRequest,
    ).length;
    await secondViewer.reload({ waitUntil: "domcontentloaded" });
    await expect(
      secondViewer.getByText(replyMarker!, { exact: false }),
    ).toBeVisible({ timeout: 30_000 });
    await expect(
      secondViewer.getByRole("button", {
        name: "Project Chat Agent",
        exact: true,
      }),
    ).toContainText("Code Agent");
    await secondViewer.waitForTimeout(750);
    const secondHistoryReadsAfterStableReload = secondViewerRequests.filter(
      (entry) => entry === historyRequest,
    ).length;
    expect(
      secondHistoryReadsAfterStableReload -
        secondHistoryReadsBeforeStableReload,
    ).toBeLessThanOrEqual(8);

    const canonicalSubmit = `POST ${apiPath("/console/submit")}`;
    const canonicalLive = `POST ${apiPath("/console/live")}`;
    const canonicalAgentSwitch = `PATCH ${apiPath("/agent-selection")}`;
    expect(controllerRequests).toContain(canonicalSubmit);
    expect(controllerRequests).toContain(canonicalLive);
    expect(controllerRequests).toContain(canonicalAgentSwitch);
    expect(secondViewerRequests).toContain(canonicalLive);
    expect(
      controllerRequests.filter((entry) => entry === historyRequest).length,
    ).toBeLessThanOrEqual(12);
    expect(
      [...controllerRequests, ...secondViewerRequests].some(
        (entry) =>
          entry.includes("/api/agents/") &&
          (entry.endsWith("/console/submit") ||
            entry.endsWith("/console/live")),
      ),
    ).toBe(false);

    await controller.screenshot({
      path: testInfo.outputPath("project-three-server-controller.png"),
      fullPage: false,
    });
    await secondViewer.screenshot({
      path: testInfo.outputPath("project-three-server-second-viewer.png"),
      fullPage: false,
    });
  } finally {
    await secondViewerContext.close();
    await controllerContext.close();
  }
});
