import { expect, test } from "@playwright/test";

// Real Chat renderer and projection, synthetic history only: no backend/provider.
const fixtureHtml = String.raw`<!doctype html><html><head>
<style>
  body { margin: 0; }
  #scroll { height: calc(100vh - 64px); overflow: auto; container-type: inline-size; }
  input { height: 40px; width: 90%; margin: 8px; }
</style></head><body><div id="root"></div><script type="module">
  import RefreshRuntime from '/@react-refresh';
  RefreshRuntime.injectIntoGlobalHook(window);
  window.$RefreshReg$ = () => {};
  window.$RefreshSig$ = () => type => type;
  window.__vite_plugin_react_preamble_installed__ = true;
  const [{default: React}, {default: ReactDOM}, {default: MessageList}, {createConversationProjector}, {ModalProvider}] = await Promise.all([
    import('/node_modules/.vite/deps/react.js'),
    import('/node_modules/.vite/deps/react-dom_client.js'),
    import('/src/pages/Chat/owned/ConversationMessageList.tsx'),
    import('/src/pages/Chat/owned/projection.ts'),
    import('/src/ui/Modal/index.tsx'),
    import('/src/styles/theme.css'),
  ]);
  const turnCount = 80;
  const toolStep = (prefix, step) => [
    {id: prefix + '-tool-' + step, role: 'assistant', type: 'plugin_call', content: [
      {type: 'tool', data: {call_id: prefix + '-call-' + step, name: 'Read', arguments: {path: 'fixture.txt'}}},
    ]},
    {id: prefix + '-result-' + step, role: 'tool', type: 'plugin_call_output', content: [
      {type: 'tool_result', data: {call_id: prefix + '-call-' + step, output: {text: 'FULL TOOL RESULT ' + prefix + ' ' + 'retained data '.repeat(500)}}},
    ]},
  ];
  const history = Array.from({length: turnCount}, (_, index) => [
    {id: 'user-' + index, role: 'user', turn_id: 'turn-' + index, content: 'Question ' + index},
    {id: 'reasoning-' + index, role: 'assistant', turn_id: 'turn-' + index, type: 'reasoning',
      content: 'Reasoning needle-' + index + '\n\n' + 'A **complete** historical thought with details.\n\n'.repeat(30)},
    ...Array.from({length: 4}, (_, step) => toolStep('history-' + index, step)).flat(),
    {id: 'answer-' + index, role: 'assistant', content: '## Answer ' + index + '\n\nVisible historical answer.'},
  ]).flat();
  history.push({id: 'live-user', role: 'user', turn_id: 'live-turn', content: 'Continue'});
  const project = createConversationProjector();
  const started = performance.now();
  window.commitDurations = [];
  function Harness() {
    const [stepCount, setStepCount] = React.useState(1);
    const [text, setText] = React.useState('');
    const [active, setActive] = React.useState(true);
    const [draft, setDraft] = React.useState('');
    const messages = React.useMemo(() => [
      ...history,
      {id: 'live-reasoning', role: 'assistant', turn_id: 'live-turn', type: 'reasoning', content: 'Live thought retained in full'},
      ...Array.from({length: stepCount}, (_, step) => toolStep('live', step)).flat(),
      ...(text ? [{id: 'live-text', role: 'assistant', type: 'message', status: active ? 'in_progress' : 'completed', content: text}] : []),
    ], [stepCount, text, active]);
    const projection = React.useMemo(() => project('render-fixture', {
      messages,
      active_turn: active ? {turn_id: 'live-turn', user_message_id: 'live-user', phase: 'streaming', driver: 'rua'} : null,
    }, {liveActive: active, hasLiveProjection: active}), [messages, active]);
    window.appendStep = () => setStepCount(count => count + 1);
    window.setLiveText = setText;
    window.finish = () => setActive(false);
    React.useEffect(() => {
      window.ready = true;
      window.initialRenderMs ??= performance.now() - started;
    }, []);
    return React.createElement(React.Fragment, null,
      React.createElement('div', {id: 'scroll'},
        React.createElement(MessageList, {projection})),
      React.createElement('input', {'aria-label': 'Draft', value: draft, onChange: event => setDraft(event.target.value)}));
  }
  ReactDOM.createRoot(document.getElementById('root')).render(
    React.createElement(ModalProvider, null,
      React.createElement(React.Profiler, {id: 'chat', onRender: (_id, _phase, duration) => window.commitDurations.push(duration)},
        React.createElement(Harness))));
</script></body></html>`;

test.beforeEach(async ({ page, request }) => {
  const viteClient = await request.get("/@vite/client");
  test.skip(
    !viteClient.headers()["content-type"]?.includes("javascript"),
    "Component browser fixture requires the Vite dev server",
  );
  // Never let a component fixture reach a real backend.
  await page.route(
    (url) => url.pathname.startsWith("/api/"),
    (route) => route.abort(),
  );
  await page.route("**/__owned-rendering-test", (route) =>
    route.fulfill({ contentType: "text/html", body: fixtureHtml }),
  );
  await page.goto("/__owned-rendering-test");
  await expect.poll(() => page.evaluate("window.ready")).toBe(true);
});

test("long folded history stays lazy while streaming preserves interaction and tool state", async ({
  page,
}, testInfo) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  const groups = page.locator(".rua-tool-group");
  await expect(groups).toHaveCount(81);
  await expect(groups.last()).toHaveClass(/is-generating/);
  await expect(page.locator(".rua-tool-group-content")).toHaveCount(0);
  await expect(page.locator(".rua-local-code-block")).toHaveCount(0);
  const initial = await page.evaluate(`({
    elements: document.querySelectorAll('#root *').length,
    renderMs: window.initialRenderMs
  })`);
  const liveGroup = groups.last();
  await liveGroup.locator(".rua-tool-group-summary").click();
  const tool = liveGroup.locator('[data-rua-tool-call-id="live-call-0"]');
  await tool.locator("summary").focus();
  await page.keyboard.press("Enter");
  await expect(tool).toHaveAttribute("open", "");
  const output = tool.getByLabel("Output code block");
  await expect(output).toContainText("FULL TOOL RESULT live");
  await output.getByRole("button", { name: "Collapse block" }).click();
  await page.evaluate(`{
    window.liveGroupNode = document.querySelectorAll('.rua-tool-group')[80];
    window.toolNode = document.querySelector('[data-rua-tool-call-id="live-call-0"]');
    const bubble = document.querySelector('.rua-chat-bubble--assistant');
    const measure = bubble.getBoundingClientRect.bind(bubble);
    window.historicalMeasurements = 0;
    bubble.getBoundingClientRect = () => { window.historicalMeasurements++; return measure(); };
    window.commitDurations.length = 0;
    window.appendStep();
  }`);
  await expect(liveGroup.locator(".rua-local-tool")).toHaveCount(2);
  const streaming = page.evaluate(async () => {
    for (let index = 1; index <= 30; index++) {
      await new Promise<void>((resolve) =>
        requestAnimationFrame(() => resolve()),
      );
      (
        window as unknown as { setLiveText: (text: string) => void }
      ).setLiveText(
        "## Live answer\n\n" + "Streaming **complete** text. ".repeat(index),
      );
    }
  });
  await page
    .getByRole("textbox", { name: "Draft" })
    .fill("Typing during output");
  await streaming;
  await expect(page.getByRole("textbox", { name: "Draft" })).toHaveValue(
    "Typing during output",
  );
  await page.evaluate("window.finish()");
  await expect(liveGroup).not.toHaveClass(/is-generating/);
  await expect(
    page
      .locator(".rua-chat-message--assistant")
      .last()
      .locator(".rua-chat-markdown")
      .last(),
  ).toHaveText("Live answer " + "Streaming complete text. ".repeat(30).trim());
  await expect(output).toHaveAttribute("data-collapsed", "true");
  expect(
    await page.evaluate(
      "window.liveGroupNode === document.querySelectorAll('.rua-tool-group')[80]",
    ),
  ).toBe(true);
  expect(
    await page.evaluate(
      "window.toolNode === document.querySelector('[data-rua-tool-call-id=\"live-call-0\"]')",
    ),
  ).toBe(true);
  expect(await page.evaluate("window.historicalMeasurements")).toBe(0);
  await expect(page.locator(".rua-tool-group-content")).toHaveCount(1);
  expect(errors).toEqual([]);
  await testInfo.attach("rendering-measurements", {
    contentType: "application/json",
    body: JSON.stringify({
      initial,
      commitMs: await page.evaluate("window.commitDurations"),
    }),
  });
});

test("search reveals hidden thinking and opening or closing a history group preserves its scroll position", async ({
  page,
}) => {
  await page.evaluate(`{
    sessionStorage.setItem('rua_search_scroll_target', JSON.stringify({cardIndex: 61, query: 'needle-30'}));
    window.dispatchEvent(new Event('rua-search-scroll'));
  }`);
  const group = page.locator(".rua-tool-group").nth(30);
  await expect(group.locator("mark.rua-search-jump-mark")).toHaveText(
    "needle-30",
  );
  await expect(group.locator(".rua-local-reasoning")).toHaveAttribute(
    "open",
    "",
  );
  await expect(group.locator(".rua-local-tool-body")).toHaveCount(0);
  const summary = group.locator(".rua-tool-group-summary");
  // Put the same heading in the viewport before each toggle.
  await summary.evaluate((node) => node.scrollIntoView({ block: "center" }));
  const beforeClose = (await summary.boundingBox())!.y;
  await summary.click();
  await expect(group.locator(".rua-tool-group-content")).toHaveCount(0);
  expect(Math.abs((await summary.boundingBox())!.y - beforeClose)).toBeLessThan(
    2,
  );
  const beforeOpen = (await summary.boundingBox())!.y;
  await summary.click();
  await expect(group.locator(".rua-local-reasoning")).toBeVisible();
  expect(Math.abs((await summary.boundingBox())!.y - beforeOpen)).toBeLessThan(
    2,
  );
});
