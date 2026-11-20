import type { Page } from "@playwright/test";

export async function mockConsoleTransport(page: Page) {
  await page.routeWebSocket("**/api/console/transport", (socket) => {
    socket.onMessage((message) => {
      const frame = JSON.parse(String(message));
      if (frame.type === "auth")
        socket.send(JSON.stringify({ type: "ready", version: 1 }));
      if (frame.type !== "open") return;
      const snapshot =
        frame.path.includes("/approvals/") ||
        frame.path.includes("/permissions");
      socket.send(
        JSON.stringify({
          type: "head",
          id: frame.id,
          status: 200,
          content_type: snapshot ? "application/json" : "text/event-stream",
        }),
      );
      if (snapshot) {
        socket.send(
          JSON.stringify({
            type: "data",
            id: frame.id,
            data: Buffer.from('{"pending":[],"resolved":[]}').toString(
              "base64",
            ),
          }),
        );
        socket.send(JSON.stringify({ type: "end", id: frame.id }));
      }
    });
  });
}
