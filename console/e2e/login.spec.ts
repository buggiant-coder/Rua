import { test, expect, goChat } from "./fixtures";
import type { APIRequestContext, Page } from "@playwright/test";

const username = "e2e-admin";
const password = "correct-horse-battery-staple";

async function authStatus(request: APIRequestContext, baseURL?: string) {
  const base = baseURL ?? "http://127.0.0.1:8090";
  const response = await request.get(`${base}/api/auth/status`);
  expect(response.ok()).toBeTruthy();
  return response.json() as Promise<{ enabled: boolean; has_users: boolean }>;
}

async function expectStoredToken(page: Page) {
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem("rua_auth_token")), {
      timeout: 10_000,
    })
    .toBeTruthy();
  return page.evaluate(() => localStorage.getItem("rua_auth_token"));
}

test.describe("login", () => {
  test("protected routes redirect unauthenticated users to login", async ({
    page,
    request,
    baseURL,
  }) => {
    const status = await authStatus(request, baseURL);
    test.skip(!status.enabled, "login e2e requires RUA_AUTH_ENABLED=true");

    await page.goto("/debug", { waitUntil: "domcontentloaded" });
    await expect(page).toHaveURL(/\/login\?redirect=%2Fdebug$/);
    await expect
      .poll(() => page.evaluate(() => localStorage.getItem("rua_auth_token")))
      .toBeNull();
    await expect(page.getByPlaceholder(/Username|用户名/)).toBeVisible();
    await expect(page.getByPlaceholder(/Password|密码/)).toBeVisible();
  });

  test("login form validates required credentials before submit", async ({
    page,
    request,
    baseURL,
  }) => {
    const status = await authStatus(request, baseURL);
    test.skip(!status.enabled, "login e2e requires RUA_AUTH_ENABLED=true");

    await page.goto("/login?redirect=/chat", { waitUntil: "domcontentloaded" });
    await page
      .getByRole("button", { name: /Register|注册|Login|登录/ })
      .click();

    await expect(
      page.getByText(/Please enter your username|请输入用户名/),
    ).toBeVisible();
    await expect(
      page.getByText(/Please enter your password|请输入密码/),
    ).toBeVisible();
    await expect
      .poll(() => page.evaluate(() => localStorage.getItem("rua_auth_token")))
      .toBeNull();
  });

  test("first user can register and then sign in with the same credentials", async ({
    page,
    request,
    baseURL,
  }) => {
    const status = await authStatus(request, baseURL);
    test.skip(!status.enabled, "login e2e requires RUA_AUTH_ENABLED=true");
    test.skip(
      status.has_users,
      "login e2e expects an isolated auth store with no existing users",
    );

    await page.goto("/login?redirect=/chat", { waitUntil: "domcontentloaded" });
    await expect(
      page.getByRole("heading", { name: /Create Account|创建账户/ }),
    ).toBeVisible();

    await page.getByPlaceholder(/Username|用户名/).fill(username);
    await page.getByPlaceholder(/Password|密码/).fill(password);
    await page.getByRole("button", { name: /Register|注册/ }).click();

    const registrationToken = await expectStoredToken(page);
    await goChat(page);
    expect(registrationToken).toBeTruthy();

    await page.evaluate(() => localStorage.removeItem("rua_auth_token"));
    await page.goto("/login?redirect=/chat", { waitUntil: "domcontentloaded" });
    await expect(
      page.getByRole("heading", { name: /Login to Rua|登录 Rua/ }),
    ).toBeVisible();

    await page.getByPlaceholder(/Username|用户名/).fill(username);
    await page.getByPlaceholder(/Password|密码/).fill(password);
    await page.getByRole("button", { name: /Login|登录/ }).click();

    const loginToken = await expectStoredToken(page);
    await goChat(page);
    expect(loginToken).toBeTruthy();
  });
});
