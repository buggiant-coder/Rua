import { expect, test } from "@playwright/test";

// Runs against Vite, without a backend or mutations to real Chats.
test("user rollback menu escapes message stacking contexts", async ({
  page,
  request,
}) => {
  const viteClient = await request.get("/@vite/client");
  test.skip(
    !viteClient.headers()["content-type"]?.includes("javascript"),
    "Component browser fixture requires the Vite dev server",
  );
  await page.route("**/__rollback-menu-test", (route) =>
    route.fulfill({
      contentType: "text/html",
      body: `<!doctype html><html><body><div id="root"></div>
        <script type="module">
          import RefreshRuntime from '/@react-refresh';
          RefreshRuntime.injectIntoGlobalHook(window);
          window.$RefreshReg$ = () => {};
          window.$RefreshSig$ = () => type => type;
          window.__vite_plugin_react_preamble_installed__ = true;
          const [{default: React}, {default: ReactDOM}, {default: MessageList}, {projectConversation}, {ModalProvider}] = await Promise.all([
            import('/node_modules/.vite/deps/react.js'),
            import('/node_modules/.vite/deps/react-dom_client.js'),
            import('/src/pages/Chat/owned/ConversationMessageList.tsx'),
            import('/src/pages/Chat/owned/projection.ts'),
            import('/src/ui/Modal/index.tsx'),
            import('/src/styles/theme.css'),
          ]);
          const projection = projectConversation('fixture', {status: 'idle', messages: [
            {id: 'first', role: 'user', turn_id: 'turn-first', content: 'Earlier input'},
            {id: 'wrong', role: 'user', turn_id: 'turn-wrong', content: 'Wrong input stopped before output'},
          ]});
          ReactDOM.createRoot(document.getElementById('root')).render(React.createElement(ModalProvider, null,
            React.createElement(MessageList, {projection, onRollback: (turnId) => {window.rollbackTurn = turnId;}})));
        </script></body></html>`,
    }),
  );
  await page.goto("/__rollback-menu-test");
  for (const width of [1280, 390]) {
    await page.setViewportSize({ width, height: 800 });
    const user = page.locator('[data-rua-chat-message-id="wrong"]');
    await user.hover();
    await user.click();
    await user.getByRole("button", { name: "More message actions" }).click();
    const rollback = page.getByRole("menuitem", { name: "Rollback" });
    await expect(rollback).toBeVisible();
    expect(
      await rollback.evaluate((node) => node.closest("article") === null),
    ).toBe(true);
    await page.getByRole("menuitem", { name: "Branch" }).click({ trial: true });
    await rollback.click();
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "Rollback", exact: true })
      .click();
    await expect
      .poll(() => page.evaluate("window.rollbackTurn"))
      .toBe("turn-wrong");
    await page.evaluate("window.rollbackTurn = null");
  }
});
