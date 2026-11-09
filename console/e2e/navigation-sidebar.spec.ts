import { expect, test, type Locator, type Page } from "@playwright/test";
import type { ChatHistory, ChatSpec } from "../src/api/types";
import type { ProjectDescriptor } from "../src/api/types/project";

const independentChatId = "00000000-0000-4000-8000-000000001001";
const projectChatId = "00000000-0000-4000-8000-000000001002";
const secondProjectChatId = "00000000-0000-4000-8000-000000001003";
const secondAgentChatId = "00000000-0000-4000-8000-000000001004";
const secondAgentPath = `/agents/research/chats/${secondAgentChatId}`;
const independentPath = `/agents/default/chats/${independentChatId}`;
const projectPath = `/projects/navigation-project/chats/${projectChatId}`;
const secondProjectPath = `/projects/second-project/chats/${secondProjectChatId}`;
const timestamp = "2026-10-23T00:00:00Z";
const model = { provider_id: "fixture", model: "navigation-model" };

const projects: ProjectDescriptor[] = [
  {
    project_id: "navigation-project",
    display_name: "导航改版",
    resource_device_id: "desktop",
    display_path: "/fixture/navigation",
    lifecycle: "active",
    created_at: timestamp,
    updated_at: timestamp,
  },
  {
    project_id: "second-project",
    display_name: "移动端计划",
    resource_device_id: "desktop",
    display_path: "/fixture/mobile",
    lifecycle: "active",
    created_at: timestamp,
    updated_at: timestamp,
  },
];

function chat(id: string, name: string): ChatSpec {
  return {
    id,
    name,
    user_id: "fixture",
    channel: "console",
    created_at: timestamp,
    updated_at: timestamp,
    archived: false,
    status: "idle",
  };
}

const chatLists: Record<string, ChatSpec[]> = {
  "/agents/default/chats": [chat(independentChatId, "日常讨论")],
  "/agents/research/chats": [chat(secondAgentChatId, "研究记录")],
  "/projects/navigation-project/chats": [chat(projectChatId, "侧栏交互设计")],
  "/projects/second-project/chats": [chat(secondProjectChatId, "手机验收方案")],
};

function history(path: string, lists = chatLists): ChatHistory {
  const isProject = path.startsWith("/projects/");
  const listPath = path.slice(0, path.lastIndexOf("/"));
  const name = lists[listPath]?.find((item) => path.endsWith(`/${item.id}`))
    ?.name;
  return {
    title: name,
    status: "idle",
    model,
    messages: [
      {
        id: `answer-${name}`,
        turn_id: `turn-${name}`,
        role: "assistant",
        type: "message",
        content: `${name}的已保存回复`,
      },
    ],
    meta: {
      authority_placement: {
        device_id: isProject ? "desktop" : "phone",
        authority_epoch: "0000000000000002000000000000000000000000",
      },
      ...(isProject
        ? {
            project_chat_agent_state: {
              selection: {
                agent_id: path.startsWith("/projects/second-project/")
                  ? "research"
                  : "default",
                selected_at: timestamp,
                revision: 1,
              },
              selected_agent_available: true,
              switchable: true,
              blockers: [],
            },
          }
        : {}),
    },
  };
}

// Exercise the actual App, router, selectors and composers. Every API request
// is synthetic; the isolated Vite proxy cannot reach a running Rua server.
async function mockNavigationApi(
  page: Page,
  overrides: Record<string, ChatSpec[]> = {},
  projectFixtures: ProjectDescriptor[] = projects,
) {
  const lists = structuredClone({ ...chatLists, ...overrides });
  const calls: { path: string; method: string; body?: unknown }[] = [];
  await page.addInitScript((researchChatId) => {
    localStorage.setItem("rua_last_chat_id_research", researchChatId);
    localStorage.setItem("language", "zh");
    localStorage.setItem("rua_sider_collapsed", "false");
    const activityState = window as typeof window & {
      navigationActivity?: (activity: unknown) => void;
      navigationActivityConnections?: number;
    };
    activityState.navigationActivityConnections = 0;
    const activityControllers = new Set<
      ReadableStreamDefaultController<Uint8Array>
    >();
    activityState.navigationActivity = (activity) => {
      const data = new TextEncoder().encode(`${JSON.stringify(activity)}\n`);
      for (const controller of activityControllers) controller.enqueue(data);
    };
    const originalFetch = window.fetch.bind(window);
    window.fetch = (input, init) => {
      const url =
        typeof input === "string"
          ? input
          : input instanceof URL
          ? input.href
          : input.url;
      if (url.includes("/platform/conversation-activity")) {
        return Promise.resolve(
          new Response(
            new ReadableStream<Uint8Array>({
              start(controller) {
                activityControllers.add(controller);
                activityState.navigationActivityConnections =
                  activityControllers.size;
                controller.enqueue(
                  new TextEncoder().encode(
                    `${JSON.stringify({ type: "snapshot", activities: [] })}\n`,
                  ),
                );
                init?.signal?.addEventListener(
                  "abort",
                  () => {
                    activityControllers.delete(controller);
                    activityState.navigationActivityConnections =
                      activityControllers.size;
                    controller.close();
                  },
                  { once: true },
                );
              },
              cancel() {
                activityControllers.clear();
                activityState.navigationActivityConnections = 0;
              },
            }),
            { headers: { "Content-Type": "application/x-ndjson" } },
          ),
        );
      }
      if (url.includes("/console/live")) {
        return Promise.resolve(
          new Response(
            new ReadableStream({
              start(controller) {
                controller.enqueue(new TextEncoder().encode(": connected\n\n"));
              },
            }),
            { headers: { "Content-Type": "text/event-stream" } },
          ),
        );
      }
      return originalFetch(input, init);
    };
  }, secondAgentChatId);
  await page.route(
    (url) => url.pathname.startsWith("/api/"),
    async (route) => {
      const url = new URL(route.request().url());
      const path = url.pathname.replace(/^\/api/, "");
      const method = route.request().method();
      const contentType = route.request().headers()["content-type"];
      const body = contentType?.includes("application/json")
        ? route.request().postDataJSON()
        : undefined;
      calls.push({ path, method, body });
      const json = (value: unknown) => route.fulfill({ json: value });
      if (path === "/platform/lifecycle" || path.endsWith("/events"))
        return route.fulfill({
          contentType: "text/event-stream",
          body: ": connected\n\n",
        });
      if (path === "/auth/status")
        return json({ enabled: false, has_users: false });
      if (path === "/agents")
        return json({
          agents: [
            {
              id: "default",
              name: "默认智能体",
              enabled: true,
              workspace_dir: "/fixture",
            },
            {
              id: "research",
              name: "研究智能体",
              enabled: true,
              workspace_dir: "/fixture/research",
            },
          ],
        });
      if (path === "/projects") return json({ projects: projectFixtures });
      const project = projectFixtures.find(
        (item) => path === `/projects/${item.project_id}`,
      );
      if (project) return json(project);
      if (lists[path]) {
        const archived = url.searchParams.get("archived");
        const visible = lists[path].filter(
          (entry) =>
            archived === null ||
            Boolean(entry.archived) === (archived === "true"),
        );
        return route.fulfill({
          json: visible,
          headers: { "X-Total-Count": String(visible.length) },
        });
      }
      if (path.endsWith("/chats/search")) {
        const matching = lists[path.slice(0, -"/search".length)] || [];
        return json({
          results: matching.map((entry) => ({
            chat_id: entry.id,
            chat_name: entry.name,
            card_index: 0,
            role: "assistant",
            snippet: "范围内的搜索结果",
          })),
        });
      }
      const metadataPath =
        /^(\/(?:agents|projects)\/[^/]+\/chats)\/([^/]+)\/metadata$/.exec(path);
      if (metadataPath) {
        const entry = lists[metadataPath[1]]?.find(
          (candidate) => candidate.id === metadataPath[2],
        );
        return entry
          ? json(entry)
          : route.fulfill({
              status: 404,
              json: { detail: "Fixture chat not found" },
            });
      }
      const chatPath = /^(\/(?:agents|projects)\/[^/]+\/chats)\/([^/]+)$/.exec(
        path,
      );
      if (chatPath) {
        const list = lists[chatPath[1]] || [];
        const entry = list.find((candidate) => candidate.id === chatPath[2]);
        if (method === "PUT" || method === "PATCH") {
          if (!entry)
            return route.fulfill({
              status: 404,
              json: { detail: "Fixture chat not found" },
            });
          Object.assign(entry, body);
          if (entry.archived) entry.pinned = false;
          return json(entry);
        }
        if (method === "DELETE") {
          if (entry?.pinned)
            return route.fulfill({
              status: 409,
              json: { detail: "Pinned Chat cannot be deleted; Unpin it first" },
            });
          lists[chatPath[1]] = list.filter(
            (candidate) => candidate.id !== chatPath[2],
          );
          return json({ deleted: true, chat_id: chatPath[2] });
        }
        return json(history(path, lists));
      }
      if (path === "/sync/status")
        return json({ enabled: true, paired: true, peer_count: 1 });
      if (path === "/sync/devices/status")
        return json({ revocation_state: "active", is_revoked: false });
      if (path === "/sync/devices")
        return json({
          devices: [
            {
              device_id: "phone",
              friendly_name: "测试手机",
              is_self: true,
              connectivity_state: "connected",
            },
            {
              device_id: "desktop",
              friendly_name: "测试电脑",
              is_self: false,
              connectivity_state: "connected",
            },
          ],
        });
      if (path === "/models/active") return json({ default_llm: model });
      if (path === "/models")
        return json([
          {
            id: "fixture",
            name: "Fixture",
            models: [{ id: model.model, name: model.model }],
            extra_models: [],
          },
        ]);
      if (path === "/terminal/sessions") return json({ sessions: [] });
      if (path === "/terminal/profiles")
        return json({
          os: "linux",
          default_profile_id: "sh",
          profiles: [{ id: "sh", label: "Shell", kind: "shell" }],
        });
      if (path === "/sync/transfer/jobs") return json([]);
      if (path === "/sync/transfer/list") {
        const body = route.request().postDataJSON();
        return json({
          device_id: body.device_id,
          path: body.path,
          entries: [],
        });
      }
      if (path.includes("approvals") || path.includes("permissions"))
        return json({ pending: [], resolved: [] });
      if (path.endsWith("/activity"))
        return json({ status: "idle", active_turn: null });
      if (path.includes("/tasks")) return json({ tasks: [] });
      if (path.endsWith("/status")) return json({ active: false });
      if (
        /\/(channels|skills|templates|runners|commands|acp\/sessions)$/.test(
          path,
        )
      )
        return json([]);
      return json({});
    },
  );
  return calls;
}

const composer = (page: Page) => page.locator(".rua-owned-composer textarea");
const sidebar = (page: Page) => page.locator("#desktop-navigation");
const header = (page: Page) => page.locator(".rua-owned-chat-header");

async function expectSavedReply(page: Page, name: string) {
  await expect(page.locator(".rua-chat-message--assistant")).toContainText(
    `${name}的已保存回复`,
  );
  await expect(composer(page)).toBeVisible();
}

test("project groups open chats directly and keep loading scoped to expanded groups", async ({
  page,
}) => {
  const calls = await mockNavigationApi(page);
  await page.goto(independentPath);
  await expectSavedReply(page, "日常讨论");
  await expect(
    sidebar(page).getByRole("button", { name: "日常讨论", exact: true }),
  ).toBeVisible();
  await expect(
    page.locator(".rua-owned-composer .rua-chat-device-chip"),
  ).toContainText("本机");
  expect(
    calls.filter((call) => /^\/projects\/[^/]+\/chats$/.test(call.path)),
  ).toHaveLength(0);

  const projectGroup = sidebar(page).getByRole("button", {
    name: "导航改版",
    exact: true,
  });
  await expect(projectGroup).toHaveAttribute("aria-expanded", "false");
  await composer(page).fill("独立对话草稿");
  await projectGroup.click();
  await expect(page).toHaveURL(new RegExp(`${independentPath}$`));
  await sidebar(page)
    .getByRole("button", { name: "侧栏交互设计", exact: true })
    .click();
  await expect(page).toHaveURL(new RegExp(`${projectPath}$`));
  await expectSavedReply(page, "侧栏交互设计");
  await expect(
    page.locator(".rua-owned-composer .rua-chat-device-chip"),
  ).toContainText("测试电脑");
  await expect(page.locator(".rua-chat-device-chip")).toHaveAttribute(
    "aria-disabled",
    "true",
  );
  await composer(page).fill("项目对话草稿");

  await sidebar(page)
    .getByRole("button", { name: "移动端计划", exact: true })
    .click();
  await expect(page).toHaveURL(new RegExp(`${projectPath}$`));
  await sidebar(page)
    .getByRole("button", { name: "手机验收方案", exact: true })
    .click();
  await expect(page).toHaveURL(new RegExp(`${secondProjectPath}$`));
  await expectSavedReply(page, "手机验收方案");
  await sidebar(page)
    .getByRole("button", { name: "侧栏交互设计", exact: true })
    .click();
  await expect(page).toHaveURL(new RegExp(`${projectPath}$`));

  await sidebar(page)
    .getByRole("button", { name: "日常讨论", exact: true })
    .click();
  await expect(page).toHaveURL(new RegExp(`${independentPath}$`));
  await expectSavedReply(page, "日常讨论");
  await expect(composer(page)).toHaveValue("独立对话草稿");
  await sidebar(page)
    .getByRole("button", { name: "侧栏交互设计", exact: true })
    .click();
  await expect(page).toHaveURL(new RegExp(`${projectPath}$`));
  await expect(composer(page)).toHaveValue("项目对话草稿");
  expect(
    calls.filter((call) =>
      /\/(console\/stop|materialize|agent-selection)$/.test(call.path),
    ),
  ).toEqual([]);
});

test("independent chats directly follow the selected Agent without Agent subgroups", async ({
  page,
}) => {
  const calls = await mockNavigationApi(page);
  await page.goto(independentPath);
  await expectSavedReply(page, "日常讨论");
  const navigation = sidebar(page).getByRole("navigation", {
    name: "对话",
    exact: true,
  });
  await expect(
    navigation.getByRole("button", { name: "日常讨论", exact: true }),
  ).toBeVisible();
  await expect(
    navigation.getByRole("button", { name: "研究记录", exact: true }),
  ).toHaveCount(0);
  await expect(
    navigation.getByRole("button", { name: "默认智能体", exact: true }),
  ).toHaveCount(0);
  await expect(
    navigation.getByRole("button", { name: "研究智能体", exact: true }),
  ).toHaveCount(0);
  expect(
    calls.filter((call) => call.path === "/agents/research/chats"),
  ).toHaveLength(0);

  await header(page)
    .getByRole("button", { name: "切换智能体", exact: true })
    .click();
  await page
    .getByRole("button", { name: "研究智能体 切换智能体", exact: true })
    .click();
  await expect(page).toHaveURL(new RegExp(`${secondAgentPath}$`));
  await expectSavedReply(page, "研究记录");
  await expect(
    navigation.getByRole("button", { name: "研究记录", exact: true }),
  ).toBeVisible();
  await expect(
    navigation.getByRole("button", { name: "日常讨论", exact: true }),
  ).toHaveCount(0);
  await expect(
    navigation.getByRole("button", { name: "研究智能体", exact: true }),
  ).toHaveCount(0);
  await expect(
    header(page).getByRole("button", { name: "切换智能体", exact: true }),
  ).toContainText("研究智能体");

  await header(page)
    .getByRole("button", { name: "切换智能体", exact: true })
    .click();
  await page
    .getByRole("button", { name: "默认智能体 切换智能体", exact: true })
    .click();
  await expect(page).toHaveURL(new RegExp(`${independentPath}$`));
  await expect(
    navigation.getByRole("button", { name: "日常讨论", exact: true }),
  ).toBeVisible();
  await expect(
    navigation.getByRole("button", { name: "研究记录", exact: true }),
  ).toHaveCount(0);
});

test("project execution Agent controls its independent list without changing ordinary Agent preference", async ({
  page,
}) => {
  await mockNavigationApi(page);
  await page.goto(independentPath);
  await expectSavedReply(page, "日常讨论");
  await page.goto(secondProjectPath);
  await expectSavedReply(page, "手机验收方案");
  await expect(
    header(page).getByRole("button", { name: "项目对话智能体", exact: true }),
  ).toContainText("研究智能体");
  const navigation = sidebar(page).getByRole("navigation", {
    name: "对话",
    exact: true,
  });
  await expect(
    navigation.getByRole("button", { name: "研究记录", exact: true }),
  ).toBeVisible();
  await expect(
    navigation.getByRole("button", { name: "日常讨论", exact: true }),
  ).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        JSON.parse(sessionStorage.getItem("rua-agent-storage") || "{}").state
          ?.selectedAgent,
    ),
  ).toBe("default");

  await header(page)
    .getByRole("button", { name: "选择项目", exact: true })
    .click();
  await page
    .locator('[class*="projectPopoverList"]')
    .getByRole("button", { name: "独立对话", exact: true })
    .click();
  await expect(page).toHaveURL(new RegExp(`${secondAgentPath}$`));
  await expectSavedReply(page, "研究记录");
  await expect(
    navigation.getByRole("button", { name: "研究记录", exact: true }),
  ).toBeVisible();
  await expect(
    navigation.getByRole("button", { name: "日常讨论", exact: true }),
  ).toHaveCount(0);
});

test("More replaces only the sidebar and preserves the mounted conversation and draft", async ({
  page,
}) => {
  await mockNavigationApi(page);
  await page.goto(independentPath);
  await expectSavedReply(page, "日常讨论");
  await composer(page).fill("返回对话后仍在的草稿");
  const input = await composer(page).elementHandle();
  await sidebar(page)
    .getByRole("button", { name: "导航改版", exact: true })
    .click();
  await expect(
    sidebar(page).getByRole("button", { name: "侧栏交互设计", exact: true }),
  ).toBeVisible();

  const moreButton = sidebar(page).getByRole("button", {
    name: "更多",
    exact: true,
  });
  const moreBounds = await moreButton.boundingBox();
  expect(moreBounds).not.toBeNull();
  await moreButton.click();
  const allFunctions = sidebar(page).getByRole("navigation", {
    name: "全部功能",
  });
  await expect(allFunctions).toBeVisible();
  const backButton = sidebar(page).getByRole("button", {
    name: "返回对话",
    exact: true,
  });
  await expect(backButton).toBeVisible();
  expect(await backButton.boundingBox()).toEqual(moreBounds);
  await allFunctions.evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  await page.evaluate(() =>
    window.scrollTo(0, document.documentElement.scrollHeight),
  );
  expect(await backButton.boundingBox()).toEqual(moreBounds);
  await expect(
    sidebar(page).getByRole("button", { name: "日常讨论", exact: true }),
  ).toBeHidden();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page).toHaveURL(new RegExp(`${independentPath}$`));
  await expectSavedReply(page, "日常讨论");
  await expect(composer(page)).toHaveValue("返回对话后仍在的草稿");
  await page.screenshot({
    path: test.info().outputPath("more.png"),
    animations: "disabled",
  });

  await backButton.click();
  expect(await moreButton.boundingBox()).toEqual(moreBounds);
  await expect(
    sidebar(page).getByRole("button", { name: "侧栏交互设计", exact: true }),
  ).toBeVisible();
  await expect(composer(page)).toHaveValue("返回对话后仍在的草稿");
  expect(await input!.evaluate((element) => element.isConnected)).toBe(true);
  await page.screenshot({
    path: test.info().outputPath("desktop.png"),
    animations: "disabled",
  });
});

test("top selectors retain independent scope and chat management stays explicitly available", async ({
  page,
}) => {
  await mockNavigationApi(page);
  await page.goto(independentPath);
  await expectSavedReply(page, "日常讨论");
  await expect(
    header(page).getByRole("button", { name: "切换智能体", exact: true }),
  ).toBeVisible();
  await expect(
    header(page).getByRole("button", { name: "选择项目", exact: true }),
  ).toContainText("独立对话");
  await expect(page.locator(".rua-owned-chat-side-panel")).toBeHidden();
  await header(page)
    .getByRole("button", { name: "管理对话", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(
    page
      .getByRole("dialog")
      .getByRole("tab", { name: "我的对话", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Close", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toBeHidden();

  await page.goto(projectPath);
  await expectSavedReply(page, "侧栏交互设计");
  await expect(
    header(page).getByRole("button", { name: "项目对话智能体", exact: true }),
  ).toBeVisible();
  await expect(
    header(page).getByRole("button", { name: "选择项目", exact: true }),
  ).toContainText("导航改版");
  await expect(page.locator(".rua-owned-chat-side-panel")).toBeHidden();
  await header(page)
    .getByRole("button", { name: "管理对话", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(
    page
      .getByRole("dialog")
      .getByRole("tab", { name: "我的对话", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Close", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toBeHidden();

  await header(page)
    .getByRole("button", { name: "选择项目", exact: true })
    .click();
  await page
    .locator('[class*="projectPopoverList"]')
    .getByRole("button", { name: "独立对话", exact: true })
    .click();
  await expect(page).toHaveURL(new RegExp(`${independentPath}$`));
  await expectSavedReply(page, "日常讨论");
  await expect(
    header(page).getByRole("button", { name: "选择项目", exact: true }),
  ).toContainText("独立对话");
});

test("terminal and airdrop stay above More in the fixed sidebar footer", async ({
  page,
}) => {
  await mockNavigationApi(page);
  await page.goto(independentPath);
  await expectSavedReply(page, "日常讨论");
  const terminal = sidebar(page).getByRole("button", {
    name: "终端",
    exact: true,
  });
  const airdrop = sidebar(page).getByRole("button", {
    name: "隔空传送",
    exact: true,
  });
  const more = sidebar(page).getByRole("button", { name: "更多", exact: true });
  await expect(terminal).toHaveCount(1);
  await expect(airdrop).toHaveCount(1);
  await expect(more).toHaveCount(1);
  const terminalBounds = await terminal.boundingBox();
  const airdropBounds = await airdrop.boundingBox();
  const moreBounds = await more.boundingBox();
  const sidebarBounds = await sidebar(page).boundingBox();
  expect(terminalBounds!.y + terminalBounds!.height).toBeLessThanOrEqual(
    airdropBounds!.y,
  );
  expect(airdropBounds!.y + airdropBounds!.height).toBeLessThanOrEqual(
    moreBounds!.y,
  );
  expect(terminalBounds!.y).toBeGreaterThanOrEqual(
    sidebarBounds!.y + sidebarBounds!.height - 150,
  );
  expect(moreBounds!.y + moreBounds!.height).toBeLessThanOrEqual(
    sidebarBounds!.y + sidebarBounds!.height,
  );
  await sidebar(page)
    .getByRole("navigation", { name: "对话", exact: true })
    .evaluate((element) => {
      for (const child of element.querySelectorAll<HTMLElement>("*")) {
        if (child.scrollHeight > child.clientHeight)
          child.scrollTop = child.scrollHeight;
      }
    });
  expect(await terminal.boundingBox()).toEqual(terminalBounds);
  expect(await airdrop.boundingBox()).toEqual(airdropBounds);
  expect(await more.boundingBox()).toEqual(moreBounds);

  await more.click();
  const moreNavigation = sidebar(page).getByRole("navigation", {
    name: "全部功能",
    exact: true,
  });
  await expect(moreNavigation).toBeVisible();
  await expect(
    moreNavigation.getByRole("button", { name: "终端", exact: true }),
  ).toHaveCount(0);
  await expect(
    moreNavigation.getByRole("button", { name: "隔空传送", exact: true }),
  ).toHaveCount(0);
  const back = sidebar(page).getByRole("button", {
    name: "返回对话",
    exact: true,
  });
  expect(await back.boundingBox()).toEqual(moreBounds);
  await back.click();
  await terminal.click();
  await expect(page).toHaveURL(/\/terminal$/);
  await expect(page.locator("[data-terminal-runtime-host]")).toBeVisible();
  await airdrop.click();
  await expect(page).toHaveURL(/\/settings\/airdrop-transfer$/);
  await expect(
    page.getByRole("button", { name: "浏览源目录", exact: true }),
  ).toBeVisible();
  await expect(page.locator("[data-terminal-runtime-host]")).toBeHidden();
});

test("mobile drawer uses the same groups and closes after selecting a chat", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await mockNavigationApi(page);
  await page.goto(`${independentPath}?_mobile=1`);
  await expectSavedReply(page, "日常讨论");
  await page.getByRole("button", { name: "menu", exact: true }).click();
  const drawer = page.locator(".rua-nav-drawer");
  await expect(drawer).toBeVisible();
  await drawer.getByRole("button", { name: "更多", exact: true }).click();
  await expect(
    drawer.getByRole("navigation", { name: "全部功能" }),
  ).toBeVisible();
  await drawer.getByRole("button", { name: "返回对话", exact: true }).click();
  await drawer.getByRole("button", { name: "导航改版", exact: true }).click();
  await expect(drawer).toBeVisible();
  await expect(
    drawer.getByRole("button", { name: "侧栏交互设计", exact: true }),
  ).toBeVisible();
  await page.screenshot({
    path: test.info().outputPath("mobile-navigation.png"),
    animations: "disabled",
  });
  await drawer
    .getByRole("button", { name: "侧栏交互设计", exact: true })
    .click();
  await expect(page).toHaveURL(new RegExp(`${projectPath}$`));
  await expect(drawer).toBeHidden();
  await expectSavedReply(page, "侧栏交互设计");
  const projectSelector = header(page).getByRole("button", {
    name: "选择项目",
    exact: true,
  });
  await expect(projectSelector).toContainText("导航改版");
  const selectorBounds = await projectSelector.boundingBox();
  const actionBounds = await page
    .locator(".rua-owned-chat-header-actions")
    .boundingBox();
  expect(selectorBounds!.width).toBeGreaterThanOrEqual(100);
  expect(selectorBounds!.x + selectorBounds!.width).toBeLessThanOrEqual(390);
  expect(selectorBounds!.y + selectorBounds!.height).toBeLessThanOrEqual(
    actionBounds!.y + 1,
  );
  await expect(
    page.locator(".rua-owned-composer .rua-chat-device-chip"),
  ).toContainText("测试电脑");
  await page.screenshot({
    path: test.info().outputPath("mobile-conversation.png"),
    animations: "disabled",
  });
  await page.setViewportSize({ width: 320, height: 844 });
  const narrowSelectorBounds = await projectSelector.boundingBox();
  const narrowActionBounds = await page
    .locator(".rua-owned-chat-header-actions")
    .boundingBox();
  expect(narrowSelectorBounds!.width).toBeGreaterThanOrEqual(100);
  expect(
    narrowSelectorBounds!.x + narrowSelectorBounds!.width,
  ).toBeLessThanOrEqual(320);
  expect(
    narrowSelectorBounds!.y + narrowSelectorBounds!.height,
  ).toBeLessThanOrEqual(narrowActionBounds!.y + 1);
  await page.screenshot({
    path: test.info().outputPath("mobile-narrow.png"),
    animations: "disabled",
  });
});

for (const scope of ["agent", "project"] as const) {
  const listPath =
    scope === "agent"
      ? "/agents/default/chats"
      : "/projects/navigation-project/chats";
  const currentId = scope === "agent" ? independentChatId : projectChatId;
  const currentPath = `${listPath}/${currentId}`;
  const markerChats = [
    { ...chat(currentId, "无标记对话"), pinned: false, tag: "none" as const },
    {
      ...chat("00000000-0000-4000-8000-000000001011", "颜色标记对话"),
      pinned: false,
      tag: "yellow" as const,
    },
    {
      ...chat("00000000-0000-4000-8000-000000001012", "置顶对话"),
      pinned: true,
      tag: "none" as const,
    },
    {
      ...chat("00000000-0000-4000-8000-000000001013", "彩色置顶对话"),
      pinned: true,
      tag: "red" as const,
    },
  ];
  const openRowMenu = async (page: Page, name: string) => {
    await sidebar(page)
      .getByRole("button", { name: `${name} 的操作`, exact: true })
      .click();
    await expect(page.getByRole("menu")).toHaveCount(1);
    await expect(page.getByRole("menu")).toBeVisible();
    return page.getByRole("menu");
  };

  test(`${scope} row markers keep four states aligned and color edits preserve pin and conversation`, async ({
    page,
  }) => {
    const calls = await mockNavigationApi(page, { [listPath]: markerChats });
    await page.goto(currentPath);
    await expectSavedReply(page, "无标记对话");
    await composer(page).fill("颜色设置不影响当前草稿");
    const titlePositions: number[] = [];
    for (const entry of markerChats) {
      const link = sidebar(page).locator(
        `[data-chat-id="${entry.id}"] button[title="${entry.name}"]`,
      );
      await expect(link).toBeVisible();
      const marker = link.locator("[data-chat-marker]");
      await expect(marker).toHaveCount(1);
      await expect(marker).toHaveAttribute("data-pinned", String(entry.pinned));
      await expect(marker).toHaveAttribute("data-tag", entry.tag);
      await expect(marker.locator("svg")).toHaveCount(entry.pinned ? 1 : 0);
      await expect(marker.locator("[data-color-dot]")).toHaveCount(
        !entry.pinned && entry.tag !== "none" ? 1 : 0,
      );
      const position = await marker.evaluate(
        (element) => element.nextElementSibling!.getBoundingClientRect().x,
      );
      titlePositions.push(position);
    }
    expect(
      Math.max(...titlePositions) - Math.min(...titlePositions),
    ).toBeLessThanOrEqual(1);
    const pinnedMarker = sidebar(page).locator(
      `[data-chat-id="${markerChats[3].id}"] [data-chat-marker]`,
    );
    const originalColor = await pinnedMarker.evaluate(
      (element) => getComputedStyle(element).color,
    );
    const menu = await openRowMenu(page, "彩色置顶对话");
    await expect(
      menu.getByRole("button", { name: "红色", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
    await menu.getByRole("button", { name: "绿色", exact: true }).click();
    await expect(pinnedMarker).toHaveAttribute("data-tag", "green");
    await expect(pinnedMarker).toHaveAttribute("data-pinned", "true");
    await expect(pinnedMarker.locator("svg")).toHaveCount(1);
    await expect(pinnedMarker).not.toHaveCSS("color", originalColor);
    expect(
      calls.filter(
        (call) => call.method === (scope === "agent" ? "PUT" : "PATCH"),
      ),
    ).toEqual([
      {
        path: `${listPath}/${markerChats[3].id}`,
        method: scope === "agent" ? "PUT" : "PATCH",
        body: { tag: "green" },
      },
    ]);
    await expect(page).toHaveURL(new RegExp(`${currentPath}$`));
    await expect(composer(page)).toHaveValue("颜色设置不影响当前草稿");
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);
    await page.emulateMedia({ colorScheme: "dark" });
    await expect(page.locator("html")).toHaveClass(/dark-mode/);
    await expect(page.locator("html")).toHaveAttribute(
      "data-color-scheme",
      "blue",
    );
    const activeRow = sidebar(page).locator(
      `[data-chat-id="${markerChats[3].id}"]`,
    );
    const activeChat = activeRow.locator('button[title="彩色置顶对话"]');
    await activeChat.click();
    await expect(activeChat).toHaveAttribute("aria-current", "page");
    await expect(page).toHaveURL(
      new RegExp(`${listPath}/${markerChats[3].id}$`),
    );
    for (const entry of markerChats) {
      await expect(
        sidebar(page).locator(`[data-chat-id="${entry.id}"]`),
      ).toBeVisible();
    }
    const markerBounds = await pinnedMarker.boundingBox();
    expect(markerBounds!.width).toBe(22);
    expect(markerBounds!.height).toBe(22);
    if (scope === "project") {
      const chevronBounds = await sidebar(page)
        .getByRole("button", { name: "导航改版", exact: true })
        .locator("svg")
        .first()
        .boundingBox();
      expect(
        Math.abs(
          markerBounds!.x +
            markerBounds!.width / 2 -
            (chevronBounds!.x + chevronBounds!.width / 2),
        ),
      ).toBeLessThanOrEqual(1);
    }
    await expect(pinnedMarker).toHaveCSS(
      "background-color",
      "rgba(0, 0, 0, 0)",
    );
    await expect(pinnedMarker).toHaveCSS("box-shadow", "none");
    for (const edge of ["top", "right", "bottom", "left"]) {
      await expect(pinnedMarker).toHaveCSS(`border-${edge}-width`, "0px");
    }
    const selectionLeft = await activeRow.evaluate(
      (element) =>
        element.getBoundingClientRect().x +
        parseFloat(getComputedStyle(element, "::before").left),
    );
    expect(markerBounds!.x + markerBounds!.width).toBeLessThanOrEqual(
      selectionLeft,
    );
    const titleLeft = await pinnedMarker.evaluate(
      (element) => element.nextElementSibling!.getBoundingClientRect().x,
    );
    expect(titleLeft).toBe(titlePositions[3]);
    const leftTextGap = titleLeft - selectionLeft;
    expect(leftTextGap).toBeGreaterThanOrEqual(8);
    const selectionRight = await activeRow.evaluate(
      (element) =>
        element.getBoundingClientRect().right -
        parseFloat(getComputedStyle(element, "::before").right),
    );
    const menuGlyph = await activeRow
      .getByRole("button", { name: "彩色置顶对话 的操作", exact: true })
      .locator("svg")
      .boundingBox();
    expect(leftTextGap).toBeGreaterThanOrEqual(
      selectionRight - menuGlyph!.x - menuGlyph!.width,
    );
    await expect(pinnedMarker.locator("svg")).toHaveAttribute("width", "16");
    await expect(pinnedMarker.locator("svg")).toHaveCSS(
      "fill",
      await pinnedMarker.evaluate((element) => getComputedStyle(element).color),
    );
    const dot = sidebar(page).locator(
      `[data-chat-id="${markerChats[1].id}"] [data-color-dot]`,
    );
    expect((await dot.boundingBox())!.width).toBe(12);
    expect((await dot.boundingBox())!.height).toBe(12);
    const contrast = await pinnedMarker.evaluate((element) => {
      const style = getComputedStyle(element);
      const luminance = (value: string) => {
        const parts = value
          .match(/[\d.]+/g)!
          .slice(0, 3)
          .map(Number)
          .map((channel) => {
            const scaled = channel / 255;
            return scaled <= 0.04045
              ? scaled / 12.92
              : Math.pow((scaled + 0.055) / 1.055, 2.4);
          });
        return parts[0] * 0.2126 + parts[1] * 0.7152 + parts[2] * 0.0722;
      };
      const foreground = luminance(style.color);
      let backgroundColor: string | null = null;
      for (
        let ancestor: Element | null = element;
        ancestor;
        ancestor = ancestor.parentElement
      ) {
        const color = getComputedStyle(ancestor).backgroundColor;
        const components = color.match(/[\d.]+/g)?.map(Number) ?? [];
        if (
          components.length >= 3 &&
          (components.length === 3 || components[3] === 1)
        ) {
          backgroundColor = color;
          break;
        }
      }
      if (!backgroundColor)
        throw new Error("No opaque marker background found");
      const background = luminance(backgroundColor);
      return (
        (Math.max(foreground, background) + 0.05) /
        (Math.min(foreground, background) + 0.05)
      );
    });
    expect(contrast).toBeGreaterThanOrEqual(3);
    await page.screenshot({
      path: test.info().outputPath("markers.png"),
      animations: "disabled",
    });
    const activeMenu = await openRowMenu(page, "彩色置顶对话");
    await expect(
      activeMenu.getByRole("button", { name: "绿色", exact: true }),
    ).toHaveAttribute("aria-pressed", "true");
    await expect(
      activeMenu.getByRole("menuitem", { name: "归档对话", exact: true }),
    ).toBeVisible();
    await expect(
      activeMenu.getByRole("menuitem", {
        name: "先取消置顶再删除",
        exact: true,
      }),
    ).toHaveAttribute("aria-disabled", "true");
    await page.screenshot({
      path: test.info().outputPath("row-actions.png"),
      animations: "disabled",
    });
  });

  test(`${scope} row archive and deletion keep navigation scoped and protect pinned chats`, async ({
    page,
  }) => {
    const calls = await mockNavigationApi(page, { [listPath]: markerChats });
    await page.goto(currentPath);
    await expectSavedReply(page, "无标记对话");
    await composer(page).fill("删除取消后保留的草稿");
    let menu = await openRowMenu(page, "置顶对话");
    await expect(
      menu.getByRole("menuitem", { name: "先取消置顶再删除", exact: true }),
    ).toHaveAttribute("aria-disabled", "true");
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu")).toHaveCount(0);
    expect(calls.filter((call) => call.method === "DELETE")).toHaveLength(0);

    menu = await openRowMenu(page, "颜色标记对话");
    await menu.getByRole("menuitem", { name: "归档对话", exact: true }).click();
    await expect(
      sidebar(page).locator(`[data-chat-id="${markerChats[1].id}"]`),
    ).toHaveCount(0);
    expect(
      calls.filter(
        (call) => call.method === (scope === "agent" ? "PUT" : "PATCH"),
      ),
    ).toEqual([
      {
        path: `${listPath}/${markerChats[1].id}`,
        method: scope === "agent" ? "PUT" : "PATCH",
        body: { archived: true },
      },
    ]);
    await expect(page).toHaveURL(new RegExp(`${currentPath}$`));
    await expect(composer(page)).toHaveValue("删除取消后保留的草稿");

    menu = await openRowMenu(page, "无标记对话");
    await menu.getByRole("menuitem", { name: "删除", exact: true }).click();
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "取消", exact: true })
      .click();
    await expect(page.getByRole("dialog")).toBeHidden();
    expect(calls.filter((call) => call.method === "DELETE")).toHaveLength(0);
    await expect(page).toHaveURL(new RegExp(`${currentPath}$`));
    await expect(composer(page)).toHaveValue("删除取消后保留的草稿");

    menu = await openRowMenu(page, "无标记对话");
    await menu.getByRole("menuitem", { name: "删除", exact: true }).click();
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "删除", exact: true })
      .click();
    await expect(page).not.toHaveURL(new RegExp(`${currentPath}$`));
    await expect(page).toHaveURL(new RegExp(`${listPath}/[^/]+$`));
    await expect(
      sidebar(page).getByRole("button", { name: "无标记对话", exact: true }),
    ).toHaveCount(0);
    await expect(composer(page)).toHaveValue("");
    expect(calls.filter((call) => call.method === "DELETE")).toEqual([
      { path: currentPath, method: "DELETE", body: undefined },
    ]);
  });
}

type NavigationActivity = {
  address: {
    scope:
      | { kind: "agent"; agent_id: string }
      | { kind: "project"; project_id: string };
    chat_id: string;
  };
  running: boolean;
  completion?: { turn_id: string; completed_at: string; token: string };
};

async function emitNavigationActivity(
  page: Page,
  activity: NavigationActivity,
) {
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as typeof window & { navigationActivityConnections?: number })
            .navigationActivityConnections || 0,
      ),
    )
    .toBeGreaterThan(0);
  await page.evaluate((payload) => {
    const state = window as typeof window & {
      navigationActivity?: (activity: unknown) => void;
    };
    if (!state.navigationActivity)
      throw new Error("Navigation activity stream is not initialized");
    state.navigationActivity({ type: "activity", ...payload });
  }, activity);
}

async function expectStatusBesideMenu(row: Locator, label: string) {
  await row.hover();
  const status = row.getByLabel(label, { exact: true });
  await expect(status).toBeVisible();
  const statusBounds = await status.boundingBox();
  const menuBounds = await row
    .getByRole("button", { name: /的操作$/ })
    .boundingBox();
  expect(menuBounds!.x + menuBounds!.width).toBeLessThanOrEqual(
    statusBounds!.x,
  );
}

for (const scope of ["agent", "project"] as const) {
  test(`${scope} background completion changes generating to unread and opening marks it read`, async ({
    page,
  }) => {
    const targetId = scope === "agent" ? independentChatId : projectChatId;
    const listPath =
      scope === "agent"
        ? "/agents/default/chats"
        : "/projects/navigation-project/chats";
    const targetName = scope === "agent" ? "日常讨论" : "侧栏交互设计";
    const targetPath = `${listPath}/${targetId}`;
    const address: NavigationActivity["address"] = {
      scope:
        scope === "agent"
          ? { kind: "agent", agent_id: "default" }
          : { kind: "project", project_id: "navigation-project" },
      chat_id: targetId,
    };
    await mockNavigationApi(page, {
      [listPath]: [
        { ...chat(targetId, targetName), pinned: true, tag: "green" },
      ],
    });
    await page.goto(scope === "agent" ? projectPath : independentPath);
    await page.bringToFront();
    await expectSavedReply(
      page,
      scope === "agent" ? "侧栏交互设计" : "日常讨论",
    );
    if (scope === "project")
      await sidebar(page)
        .getByRole("button", { name: "导航改版", exact: true })
        .click();
    const row = sidebar(page).locator(`[data-chat-id="${targetId}"]`);
    await expect(row).toBeVisible();
    await emitNavigationActivity(page, { address, running: true });
    await expectStatusBesideMenu(row, "正在生成");
    await page.screenshot({
      path: test.info().outputPath("activity-running.png"),
      animations: "disabled",
    });
    await expect(row.locator("[data-chat-marker]")).toHaveAttribute(
      "data-pinned",
      "true",
    );
    await expect(row.locator("[data-chat-marker]")).toHaveAttribute(
      "data-tag",
      "green",
    );
    await emitNavigationActivity(page, {
      address,
      running: false,
      completion: {
        turn_id: "background-turn",
        completed_at: timestamp,
        token: "background-turn:1",
      },
    });
    await expect(row.getByLabel("正在生成", { exact: true })).toHaveCount(0);
    await expectStatusBesideMenu(row, "未读回复");
    await page.screenshot({
      path: test.info().outputPath("activity-unread.png"),
      animations: "disabled",
    });
    await row.locator(`button[title="${targetName}"]`).click();
    await expect(page).toHaveURL(new RegExp(`${targetPath}$`));
    await expectSavedReply(page, targetName);
    await expect(row.getByLabel("未读回复", { exact: true })).toHaveCount(0);
    await emitNavigationActivity(page, { address, running: true });
    await expect(row.getByLabel("正在生成", { exact: true })).toBeVisible();
    await emitNavigationActivity(page, {
      address,
      running: false,
      completion: {
        turn_id: "foreground-turn",
        completed_at: "2026-10-23T00:01:00Z",
        token: "foreground-turn:2",
      },
    });
    await expect(row.getByLabel("正在生成", { exact: true })).toHaveCount(0);
    await expect(row.getByLabel("未读回复", { exact: true })).toHaveCount(0);
  });
}

test("activity isolates identical chat IDs across Agent and Project scopes", async ({
  page,
}) => {
  const sharedId = "00000000-0000-4000-8000-000000001099";
  await mockNavigationApi(page, {
    "/agents/default/chats": [
      chat(independentChatId, "日常讨论"),
      chat(sharedId, "同ID独立对话"),
    ],
    "/projects/navigation-project/chats": [chat(sharedId, "同ID项目对话")],
  });
  await page.goto(independentPath);
  await page.bringToFront();
  await expectSavedReply(page, "日常讨论");
  await sidebar(page)
    .getByRole("button", { name: "导航改版", exact: true })
    .click();
  const agentRow = sidebar(page)
    .locator(`[data-chat-id="${sharedId}"]`)
    .filter({ has: page.locator('button[title="同ID独立对话"]') });
  const projectRow = sidebar(page)
    .locator(`[data-chat-id="${sharedId}"]`)
    .filter({ has: page.locator('button[title="同ID项目对话"]') });
  await expect(agentRow).toBeVisible();
  await expect(projectRow).toBeVisible();
  const agentAddress: NavigationActivity["address"] = {
    scope: { kind: "agent", agent_id: "default" },
    chat_id: sharedId,
  };
  const projectAddress: NavigationActivity["address"] = {
    scope: { kind: "project", project_id: "navigation-project" },
    chat_id: sharedId,
  };
  await emitNavigationActivity(page, { address: agentAddress, running: true });
  await expect(agentRow.getByLabel("正在生成", { exact: true })).toBeVisible();
  await expect(projectRow.getByLabel("正在生成", { exact: true })).toHaveCount(
    0,
  );
  await emitNavigationActivity(page, {
    address: projectAddress,
    running: true,
  });
  await expect(
    projectRow.getByLabel("正在生成", { exact: true }),
  ).toBeVisible();
  await emitNavigationActivity(page, {
    address: agentAddress,
    running: false,
    completion: {
      turn_id: "same-id-agent",
      completed_at: timestamp,
      token: "same-id-agent:1",
    },
  });
  await expect(agentRow.getByLabel("未读回复", { exact: true })).toBeVisible();
  await expect(
    projectRow.getByLabel("正在生成", { exact: true }),
  ).toBeVisible();
  await expect(projectRow.getByLabel("未读回复", { exact: true })).toHaveCount(
    0,
  );
  await emitNavigationActivity(page, {
    address: projectAddress,
    running: false,
    completion: {
      turn_id: "same-id-project",
      completed_at: timestamp,
      token: "same-id-project:1",
    },
  });
  await expect(
    projectRow.getByLabel("未读回复", { exact: true }),
  ).toBeVisible();
  await agentRow.locator('button[title="同ID独立对话"]').click();
  await expect(page).toHaveURL(
    new RegExp(`/agents/default/chats/${sharedId}$`),
  );
  await expect(agentRow.getByLabel("未读回复", { exact: true })).toHaveCount(0);
  await expect(
    projectRow.getByLabel("未读回复", { exact: true }),
  ).toBeVisible();
});

test("search scope uses the shared floating Select and searches only the chosen scope", async ({
  page,
}) => {
  const calls = await mockNavigationApi(page);
  await page.goto(independentPath);
  await expectSavedReply(page, "日常讨论");
  await sidebar(page)
    .getByRole("button", { name: "搜索对话", exact: true })
    .click();
  const scopeSelect = sidebar(page).getByRole("combobox", {
    name: "搜索范围",
    exact: true,
  });
  await expect(scopeSelect).toBeVisible();
  expect(await scopeSelect.evaluate((element) => element.tagName)).not.toBe(
    "SELECT",
  );
  await scopeSelect.click();
  await page.getByRole("option", { name: "导航改版", exact: true }).click();
  await expect(scopeSelect).toContainText("导航改版");
  await sidebar(page)
    .getByRole("searchbox", { name: "搜索对话", exact: true })
    .fill("范围查询");
  await expect(
    sidebar(page).getByRole("button", {
      name: /侧栏交互设计.*范围内的搜索结果/,
    }),
  ).toBeVisible();
  expect(calls.filter((call) => call.path.endsWith("/chats/search"))).toEqual([
    {
      path: "/projects/navigation-project/chats/search",
      method: "GET",
      body: undefined,
    },
  ]);
  await expect(page).toHaveURL(new RegExp(`${independentPath}$`));
});

test("project heading menu opens project creation and management", async ({
  page,
}) => {
  const calls = await mockNavigationApi(page);
  await page.goto(independentPath);
  await expectSavedReply(page, "日常讨论");
  await sidebar(page)
    .getByRole("button", { name: "项目操作", exact: true })
    .click();
  await page.getByRole("menuitem", { name: "新建项目", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByRole("heading", { name: "新建项目", exact: true }),
  ).toBeVisible();
  await expect(dialog.getByRole("combobox")).toContainText("测试手机");
  await dialog.getByRole("button", { name: "取消", exact: true }).click();
  await expect(dialog).toBeHidden();
  expect(
    calls.filter((call) => call.path === "/projects" && call.method === "POST"),
  ).toHaveLength(0);
  await expect(page).toHaveURL(new RegExp(`${independentPath}$`));
  await sidebar(page)
    .getByRole("button", { name: "项目操作", exact: true })
    .click();
  await page.getByRole("menuitem", { name: "管理项目", exact: true }).click();
  await expect(page).toHaveURL(/\/projects\?overview=1$/);
});

test("project heading sorting follows names creation and chat activity without changing the open chat", async ({
  page,
}) => {
  const projectFixtures: ProjectDescriptor[] = [
    {
      ...projects[0],
      project_id: "zeta",
      display_name: "Zeta",
      created_at: "2026-10-22T00:00:00Z",
      updated_at: "2026-10-23T23:00:00Z",
      last_activity_at: "2026-10-21T00:00:00Z",
    },
    {
      ...projects[0],
      project_id: "alpha",
      display_name: "Alpha",
      created_at: "2026-10-20T00:00:00Z",
      updated_at: "2026-10-22T00:00:00Z",
      last_activity_at: null,
    },
    {
      ...projects[0],
      project_id: "beta",
      display_name: "Beta",
      created_at: "2026-10-21T00:00:00Z",
      updated_at: "2026-10-21T00:00:00Z",
      last_activity_at: "2026-10-23T00:00:00Z",
    },
  ];
  const calls = await mockNavigationApi(page, {}, projectFixtures);
  await page.goto(independentPath);
  await expectSavedReply(page, "日常讨论");
  await composer(page).fill("切换项目排序保留的草稿");
  const projectNames = sidebar(page).getByRole("button", {
    name: /^(Alpha|Beta|Zeta)$/,
  });
  await expect(projectNames).toHaveText(["Beta", "Zeta", "Alpha"]);
  for (const [label, expected] of [
    ["按名称", ["Alpha", "Beta", "Zeta"]],
    ["按创建时间", ["Zeta", "Beta", "Alpha"]],
    ["按最后活动时间", ["Beta", "Zeta", "Alpha"]],
  ] as const) {
    await sidebar(page)
      .getByRole("button", { name: "项目操作", exact: true })
      .click();
    await page.getByRole("menuitem", { name: label, exact: true }).click();
    await expect(projectNames).toHaveText([...expected]);
    await expect(page).toHaveURL(new RegExp(`${independentPath}$`));
    await expect(composer(page)).toHaveValue("切换项目排序保留的草稿");
  }
  expect(
    calls.filter((call) => /^\/projects\/[^/]+\/chats$/.test(call.path)),
  ).toHaveLength(0);
});

for (const scenario of [
  "independent to project",
  "same project",
  "projects with same Agent",
  "projects with different Agents",
] as const) {
  test(`sidebar click preserves the visible chat position: ${scenario}`, async ({
    page,
  }) => {
    const independentChats = [
      chat(independentChatId, "日常讨论"),
      ...Array.from({ length: 19 }, (_, index) =>
        chat(`independent-scroll-${index}`, `独立历史 ${index + 1}`),
      ),
    ];
    const targetId = scenario.startsWith("projects")
      ? secondProjectChatId
      : projectChatId;
    const targetTitle = scenario.startsWith("projects")
      ? "手机验收方案"
      : "侧栏交互设计";
    const targetPath = scenario.startsWith("projects")
      ? secondProjectPath
      : projectPath;
    const sourcePath =
      scenario === "independent to project"
        ? independentPath
        : "/projects/navigation-project/chats/project-scroll-0";
    const projectChats = Array.from({ length: 18 }, (_, index) =>
      chat(
        index === 5 ? projectChatId : `project-scroll-${index}`,
        index === 5 ? "侧栏交互设计" : `项目历史 ${index + 1}`,
      ),
    );
    const secondChats = Array.from({ length: 18 }, (_, index) =>
      chat(
        index === 5 ? secondProjectChatId : `second-scroll-${index}`,
        index === 5 ? "手机验收方案" : `另一项目历史 ${index + 1}`,
      ),
    );
    await mockNavigationApi(page, {
      "/agents/default/chats": independentChats,
      "/agents/research/chats": Array.from({ length: 12 }, (_, index) =>
        chat(`research-scroll-${index}`, `研究历史 ${index + 1}`),
      ),
      "/projects/navigation-project/chats": projectChats,
      "/projects/second-project/chats": secondChats,
    });
    let independentLoads = 0;
    await page.route(
      (url) => url.pathname === "/api/agents/default/chats",
      async (route) => {
        independentLoads += 1;
        if (independentLoads > 1)
          await new Promise((resolve) => setTimeout(resolve, 100));
        await route.fallback();
      },
    );
    if (scenario === "projects with same Agent") {
      await page.route(
        (url) => url.pathname === `/api${targetPath}`,
        async (route) => {
          const result = history(targetPath);
          (
            result.meta!.project_chat_agent_state as {
              selection: { agent_id: string };
            }
          ).selection.agent_id = "default";
          await route.fulfill({ json: result });
        },
      );
    }
    await page.addInitScript((targetChatId) => {
      localStorage.setItem(
        "rua_navigation_expanded_groups",
        JSON.stringify([
          "independent",
          "project:navigation-project",
          "project:second-project",
        ]),
      );
      const state = window as typeof window & {
        sidebarScrollTrace: unknown[];
        sidebarScrollSampling: boolean;
      };
      state.sidebarScrollTrace = [];
      state.sidebarScrollSampling = false;
      document.addEventListener(
        "pointerdown",
        (event) => {
          if (
            !(event.target instanceof Element) ||
            !event.target.closest(`[data-chat-id="${targetChatId}"]`)
          )
            return;
          state.sidebarScrollSampling = true;
          const sample = () => {
            if (!state.sidebarScrollSampling) return;
            const element = document.querySelector<HTMLElement>(
              'nav[aria-label="对话"] div[class*="__scroll__"]',
            )!;
            const row = element.querySelector(
              `[data-chat-id="${targetChatId}"]`,
            );
            state.sidebarScrollTrace.push({
              event: "frame",
              scrollTop: element.scrollTop,
              scrollHeight: element.scrollHeight,
              rowY: row?.getBoundingClientRect().y,
            });
            requestAnimationFrame(sample);
          };
          sample();
        },
        true,
      );
    }, targetId);
    await page.goto(sourcePath);
    await expectSavedReply(
      page,
      scenario === "independent to project" ? "日常讨论" : "项目历史 1",
    );
    const target = sidebar(page).locator(
      `[data-chat-id="${targetId}"] button[title="${targetTitle}"]`,
    );
    await expect(target).toBeAttached();
    const scroll = sidebar(page)
      .getByRole("navigation", { name: "对话", exact: true })
      .locator('div[class*="__scroll__"]');
    await expect(
      sidebar(page).locator('[data-chat-id="independent-scroll-18"]'),
    ).toBeAttached();
    await expect(sidebar(page).getByRole("status")).toHaveCount(0);
    await page.evaluate(
      () =>
        new Promise<void>((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
        ),
    );
    await target.evaluate((element) => {
      const container = element
        .closest('nav[aria-label="对话"]')!
        .querySelector<HTMLElement>('div[class*="__scroll__"]')!;
      container.dispatchEvent(
        new PointerEvent("pointerdown", { bubbles: true }),
      );
      container.scrollTop +=
        element.getBoundingClientRect().top -
        container.getBoundingClientRect().top -
        250;
    });
    await expect(target).toBeInViewport();
    const before = await scroll.evaluate((element) => ({
      scrollTop: element.scrollTop,
      scrollHeight: element.scrollHeight,
      clientHeight: element.clientHeight,
    }));
    const rowBefore = await target.boundingBox();
    await page.screenshot({
      path: test.info().outputPath("sidebar-scroll-before.png"),
    });
    await page.evaluate(() => {
      (
        window as typeof window & { sidebarScrollTrace: unknown[] }
      ).sidebarScrollTrace = [];
    });
    await target.click();
    await expect(page).toHaveURL(new RegExp(`${targetPath}$`));
    await expectSavedReply(page, targetTitle);
    await expect(
      sidebar(page).locator(
        `[data-chat-id="${
          scenario === "projects with different Agents"
            ? "research-scroll-11"
            : "independent-scroll-18"
        }"]`,
      ),
    ).toBeAttached();
    await page.evaluate(
      () =>
        new Promise<void>((resolve) => {
          let frames = 0;
          const settle = () => {
            if (++frames < 12) requestAnimationFrame(settle);
            else resolve();
          };
          requestAnimationFrame(settle);
        }),
    );
    const after = await scroll.evaluate((element) => ({
      scrollTop: element.scrollTop,
      scrollHeight: element.scrollHeight,
      clientHeight: element.clientHeight,
    }));
    const rowAfter = await target.boundingBox();
    const trace = await page.evaluate(() => {
      const state = window as typeof window & {
        sidebarScrollTrace: Array<{ event: string; rowY?: number }>;
        sidebarScrollSampling: boolean;
      };
      state.sidebarScrollSampling = false;
      return state.sidebarScrollTrace;
    });
    await page.screenshot({
      path: test.info().outputPath("sidebar-scroll-after.png"),
    });
    const evidence = JSON.stringify(
      { before, after, rowBefore, rowAfter, trace },
      null,
      2,
    );
    await test.info().attach("sidebar-scroll-trace", {
      body: evidence,
      contentType: "application/json",
    });
    expect(Math.abs(rowAfter!.y - rowBefore!.y), evidence).toBeLessThanOrEqual(
      1,
    );
    const frameOffsets = trace
      .filter((entry) => entry.event === "frame" && entry.rowY !== undefined)
      .map((entry) => Math.abs(entry.rowY! - rowBefore!.y));
    expect(Math.max(...frameOffsets), evidence).toBeLessThanOrEqual(1);
    if (scenario !== "projects with different Agents")
      expect(after.scrollTop, evidence).toBe(before.scrollTop);
  });
}

for (const scope of ["agent", "project"] as const) {
  test(`open archived ${scope} chat stays highlighted without restoring or scrolling`, async ({
    page,
  }) => {
    const listPath =
      scope === "project"
        ? "/projects/navigation-project/chats"
        : "/agents/default/chats";
    const original = chatLists[listPath][0];
    const saved = {
      ...chat("00000000-0000-4000-8000-000000001099", "已归档的历史讨论"),
      archived: true,
    };
    const calls = await mockNavigationApi(page, {
      [listPath]: [original, saved],
    });
    await page.goto(`${listPath}/${saved.id}`);
    await expectSavedReply(page, saved.name!);
    const row = sidebar(page).locator(`[data-chat-id="${saved.id}"]`);
    await expect(row).toBeVisible();
    await expect(
      row.getByRole("button", { name: `${saved.name} 已归档`, exact: true }),
    ).toHaveAttribute("aria-current", "page");
    await expect(row.getByText("已归档", { exact: true })).toBeVisible();
    const activeBackground = await row.evaluate(
      (element) => getComputedStyle(element, "::before").backgroundColor,
    );
    expect(activeBackground).not.toBe("rgba(0, 0, 0, 0)");
    expect(
      calls.filter(
        (call) =>
          call.path === `${listPath}/${saved.id}/metadata` &&
          call.method === "GET",
      ).length,
    ).toBeGreaterThan(0);
    expect(
      calls.filter((call) => call.method === "PUT" || call.method === "PATCH"),
    ).toHaveLength(0);
    await row.getByRole("button", { name: `${saved.name} 的操作` }).click();
    await expect(
      page.getByRole("menuitem", { name: "管理对话" }),
    ).toBeVisible();
    await expect(page.getByRole("menuitem", { name: "归档对话" })).toHaveCount(
      0,
    );
    await page.keyboard.press("Escape");
    const next = sidebar(page).getByRole("button", {
      name: original.name!,
      exact: true,
    });
    const before = await next.boundingBox();
    await next.click();
    await expect(page).toHaveURL(new RegExp(`${listPath}/${original.id}$`));
    await expect(row).toHaveCount(0);
    const after = await next.boundingBox();
    expect(Math.abs(after!.y - before!.y)).toBeLessThan(2);
    await expect(next).toHaveAttribute("aria-current", "page");
  });
}
