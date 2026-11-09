import { test, expect } from "./fixtures";
import type { APIRequestContext, Page } from "@playwright/test";

type HeartbeatConfig = {
  enabled?: boolean;
  every?: string;
  target?: string;
  activeHours?: { start: string; end: string } | null;
};

function apiBase(baseURL?: string): string {
  return baseURL ?? "http://127.0.0.1:8090";
}

async function getHeartbeatConfig(
  request: APIRequestContext,
  baseURL: string | undefined,
): Promise<HeartbeatConfig> {
  const response = await request.get(
    `${apiBase(baseURL)}/api/config/heartbeat`,
  );
  expect(response.ok()).toBeTruthy();
  return (await response.json()) as HeartbeatConfig;
}

async function setHeartbeatConfig(
  request: APIRequestContext,
  baseURL: string | undefined,
  config: HeartbeatConfig,
): Promise<void> {
  const response = await request.put(
    `${apiBase(baseURL)}/api/config/heartbeat`,
    {
      data: config,
    },
  );
  expect(response.ok()).toBeTruthy();
}

async function chooseSelectOption(
  page: Page,
  selectIndex: number,
  option: RegExp,
): Promise<void> {
  await page.locator(".rua-ui-select").nth(selectIndex).click();
  const dropdown = page.locator(".rua-ui-select-dropdown").last();
  await expect(dropdown).toBeVisible();
  await dropdown.getByText(option).click();
}

async function chooseTime(
  page: Page,
  pickerIndex: number,
  hour: string,
  minute: string,
): Promise<void> {
  await page.locator(".rua-ui-timepicker-input").nth(pickerIndex).click();
  await page
    .getByRole("listbox", { name: "Hour" })
    .getByRole("option", { name: hour })
    .click();
  await page
    .getByRole("listbox", { name: "Minute" })
    .getByRole("option", { name: minute })
    .click();
  await page.mouse.click(0, 0);
}

test.describe("heartbeat", () => {
  test("heartbeat page reflects and persists Rust heartbeat config", async ({
    page,
    request,
    baseURL,
  }) => {
    const original = await getHeartbeatConfig(request, baseURL);

    try {
      await setHeartbeatConfig(request, baseURL, {
        enabled: false,
        every: "15m",
        target: "main",
        activeHours: { start: "08:00", end: "22:00" },
      });

      await page.goto("/heartbeat", { waitUntil: "domcontentloaded" });
      await expect(page.getByText(/Heartbeat|心跳/).first()).toBeVisible();
      await expect(page.locator(".rua-number-input")).toHaveValue("15");
      await expect(page.getByText(/Minutes|分钟/).first()).toBeVisible();
      await expect(
        page.getByText(/Silent mode|静默运行/).first(),
      ).toBeVisible();
      await expect(
        page.locator(".rua-ui-timepicker-input").nth(0),
      ).toHaveValue("08:00");
      await expect(
        page.locator(".rua-ui-timepicker-input").nth(1),
      ).toHaveValue("22:00");

      await page.locator(".rua-toggle").first().click();
      await expect
        .poll(
          async () => (await getHeartbeatConfig(request, baseURL)).enabled,
          { timeout: 10_000 },
        )
        .toBe(true);

      await page.reload({ waitUntil: "domcontentloaded" });
      await expect(page.locator(".rua-toggle-input").first()).toBeChecked();
    } finally {
      await setHeartbeatConfig(request, baseURL, original);
    }
  });

  test("heartbeat page saves interval target and active hours through Rust API", async ({
    page,
    request,
    baseURL,
  }) => {
    const original = await getHeartbeatConfig(request, baseURL);

    try {
      await setHeartbeatConfig(request, baseURL, {
        enabled: true,
        every: "10m",
        target: "main",
        activeHours: null,
      });

      await page.goto("/heartbeat", { waitUntil: "domcontentloaded" });
      await expect(page.getByText(/Heartbeat|心跳/).first()).toBeVisible();

      await page.locator(".rua-number-input").fill("2");
      await chooseSelectOption(page, 0, /Hours|小时/);
      await chooseSelectOption(page, 1, /last chat|上次对话/);
      await page.locator(".rua-toggle").nth(1).click();
      await chooseTime(page, 0, "06", "15");
      await chooseTime(page, 1, "23", "45");
      await page.getByRole("button", { name: /Save|保存/ }).click();

      await expect
        .poll(
          async () => {
            const config = await getHeartbeatConfig(request, baseURL);
            return {
              activeHours: config.activeHours,
              every: config.every,
              target: config.target,
            };
          },
          { timeout: 10_000 },
        )
        .toEqual({
          activeHours: { start: "06:15", end: "23:45" },
          every: "2h",
          target: "last",
        });

      await page.reload({ waitUntil: "domcontentloaded" });
      await expect(page.locator(".rua-number-input")).toHaveValue("2");
      await expect(
        page.locator(".rua-ui-timepicker-input").nth(0),
      ).toHaveValue("06:15");
      await expect(
        page.locator(".rua-ui-timepicker-input").nth(1),
      ).toHaveValue("23:45");
    } finally {
      await setHeartbeatConfig(request, baseURL, original);
    }
  });
});
