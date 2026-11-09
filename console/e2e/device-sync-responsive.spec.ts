import { expect, test, type Page } from "@playwright/test";

const effectivePolicy = {
  mode: "power_saver",
  periodic_interval_secs: 600,
  background_flush_grace_secs: 60,
  allow_background_runtime: false,
  allow_background_scheduled_sync: false,
};

async function mockDeviceSyncApis(page: Page) {
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname.replace(/^\/api/, "");
    const now = Date.now();

    const json = (body: unknown) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(body),
      });

    if (path === "/sync/config") {
      return json({ enabled: true });
    }
    if (path === "/sync/status") {
      return json({
        enabled: true,
        paired: true,
        peer_count: 2,
        host_profile: "mobile-apk",
        effective_power_policy: effectivePolicy,
      });
    }
    if (path === "/sync/power-policy") {
      return json({
        host_profile: "mobile-apk",
        stored_policy: null,
        effective_policy: effectivePolicy,
      });
    }
    if (path === "/sync/devices") {
      return json({
        devices: [
          {
            device_id: "dev-1",
            public_key: "peer-1",
            friendly_name: "Desk Studio",
            label: "desktop",
            added_at_ms: now - 86_400_000,
            last_seen_ms: now - 30_000,
            is_self: false,
          },
          {
            device_id: "dev-2",
            public_key: "peer-2",
            friendly_name: "Mobile Phone With Long Device Name",
            label: "android",
            added_at_ms: now - 86_400_000,
            last_seen_ms: now - 60_000,
            is_self: false,
          },
        ],
      });
    }
    if (path === "/sync/diagnostics") {
      return json({
        host_profile: "mobile-apk",
        effective_policy: effectivePolicy,
        runtime_state: {
          state: "active",
          pending_grace: false,
          background_grace: { pending: false },
          warm_suspended: false,
        },
        inbox: {
          counts: {
            pending: 1,
            retryable: 0,
            dormant: 0,
            applied: 4,
            failed: 0,
          },
        },
        progress: {
          state: "applying_changes",
          is_caught_up: false,
          pending_inbound: 1,
          retryable_inbound: 0,
          dormant_inbound: 0,
          failed_inbound: 0,
          pending_conflicts: 0,
        },
        transport: {
          last_cycle: {
            completed_at_ms: now,
            peer_node_id: "peer-1",
            status: "success",
            events_received: 8,
            events_sent: 5,
            connect_ms: 80,
            process_ms: 240,
            duration_ms: 320,
            route: "mixed",
            direct_tx_bytes: 512,
            direct_rx_bytes: 512,
            relay_tx_bytes: 1536,
            relay_rx_bytes: 1536,
            direct_rtt_ms: 42,
            relay_rtt_ms: 120,
          },
          rolling: {
            cycles: 4,
            failures: 0,
            events_received: 8,
            events_sent: 5,
            direct_tx_bytes: 512,
            direct_rx_bytes: 512,
            relay_tx_bytes: 1536,
            relay_rx_bytes: 1536,
          },
        },
        peers: {
          total_peers: 2,
          selectable_peers: 2,
          peers_in_cooldown: 0,
          peers: [
            {
              node_id: "peer-1",
              device_id_hex: "dev-1",
              last_seen_or_added_ms: now,
              selectable: true,
              consecutive_failures: 0,
              last_success_ms: now,
              sync_timeout_ms: 30000,
              last_route: "mixed",
              last_direct_bytes: 1024,
              last_relay_bytes: 3072,
              last_rtt_ms: 42,
            },
          ],
        },
      });
    }
    if (path === "/sync/conflicts/recent") {
      return json({ conflicts: [], count: 0 });
    }
    if (path === "/sync/conflict-policy") {
      return json({ policy: "silent" });
    }
    if (path === "/auth/status") {
      return json({ enabled: false, has_users: false });
    }

    return json({});
  });
}

async function openDeviceSync(page: Page, width: number) {
  await page.setViewportSize({ width, height: 900 });
  await mockDeviceSyncApis(page);
  await page.goto("/settings/device-sync", { waitUntil: "networkidle" });
  await expect(page.getByText("Device Sync").first()).toBeVisible();
  await expect(page.getByText("Applying changes")).toBeVisible();
  await expect(
    page.getByText("Mobile Phone With Long Device Name"),
  ).toBeVisible();
}

async function expectNoHorizontalPageScroll(page: Page) {
  const metrics = await page.evaluate(() => ({
    documentScrollWidth: document.documentElement.scrollWidth,
    documentClientWidth: document.documentElement.clientWidth,
    bodyScrollWidth: document.body.scrollWidth,
    bodyClientWidth: document.body.clientWidth,
  }));

  expect(metrics.documentScrollWidth).toBeLessThanOrEqual(
    metrics.documentClientWidth + 1,
  );
  expect(metrics.bodyScrollWidth).toBeLessThanOrEqual(
    metrics.bodyClientWidth + 1,
  );
}

test.describe("Device Sync responsive progress layout", () => {
  test("desktop and phone widths keep progress and device rows readable", async ({
    page,
  }) => {
    await openDeviceSync(page, 1440);
    await expectNoHorizontalPageScroll(page);
    await expect(page.getByText("Relay ratio")).toBeVisible();
    await expect(page.getByText("Mixed").first()).toBeVisible();

    await openDeviceSync(page, 390);
    await expectNoHorizontalPageScroll(page);
    await expect(page.getByText("Relay ratio")).toBeVisible();
    await expect(page.getByText("1 pending")).toBeVisible();
    await expect(page.getByTestId("remove-device-dev-2")).toBeVisible();
  });
});
