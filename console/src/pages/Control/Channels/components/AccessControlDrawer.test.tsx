import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AccessControlDrawer } from "./AccessControlDrawer";

const apiMock = vi.hoisted(() => ({
  getAclAll: vi.fn(),
  getAclAllPending: vi.fn(),
  approveAclPending: vi.fn(),
  denyAclPending: vi.fn(),
  dismissAclPending: vi.fn(),
  updatePendingRemark: vi.fn(),
  updateAclRemark: vi.fn(),
  updateUsername: vi.fn(),
  addAclWhitelist: vi.fn(),
  addAclBlacklist: vi.fn(),
  removeAclWhitelist: vi.fn(),
  removeAclBlacklist: vi.fn(),
}));

vi.mock("../../../../api", () => ({
  default: apiMock,
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (_key: string, options?: { defaultValue?: string; count?: number }) =>
      (options?.defaultValue ?? _key).replace(
        "{{count}}",
        String(options?.count ?? ""),
      ),
  }),
}));

vi.mock("@/ui", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ui")>();
  return {
    ...actual,
    toast: {
      success: vi.fn(),
      error: vi.fn(),
      info: vi.fn(),
      warning: vi.fn(),
    },
  };
});

function mockAclSnapshot() {
  apiMock.getAclAll.mockResolvedValue({
    telegram: {
      whitelist: {
        trusted: { username: "Trusted", remark: "team" },
      },
      blacklist: {},
      pending: [],
    },
    feishu: {
      whitelist: {},
      blacklist: {
        blocked: { username: "Blocked", remark: "spam" },
      },
      pending: [],
    },
  });
  apiMock.getAclAllPending.mockResolvedValue([
    {
      channel: "telegram",
      user_id: "pending-1",
      username: "Alice",
      remark: "candidate",
      first_message: "hello",
      timestamp: 1,
    },
  ]);
}

function renderDrawer() {
  return render(
    <AccessControlDrawer open agentId="agent-1" onClose={vi.fn()} />,
  );
}

describe("AccessControlDrawer", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockAclSnapshot();
    apiMock.approveAclPending.mockResolvedValue({});
    apiMock.denyAclPending.mockResolvedValue({});
    apiMock.dismissAclPending.mockResolvedValue({});
    apiMock.updatePendingRemark.mockResolvedValue({});
    apiMock.updateAclRemark.mockResolvedValue({});
    apiMock.updateUsername.mockResolvedValue({});
    apiMock.addAclWhitelist.mockResolvedValue({});
    apiMock.addAclBlacklist.mockResolvedValue({});
    apiMock.removeAclWhitelist.mockResolvedValue({});
    apiMock.removeAclBlacklist.mockResolvedValue({});
  });

  it("loads agent-scoped pending users and approves selected rows in batch", async () => {
    renderDrawer();

    await screen.findByText("pending-1");
    expect(apiMock.getAclAll).toHaveBeenCalledWith("agent-1");
    expect(apiMock.getAclAllPending).toHaveBeenCalledWith("agent-1");

    fireEvent.click(screen.getAllByLabelText("Select user")[0]);
    fireEvent.click(screen.getByRole("button", { name: "Approve selected" }));

    await waitFor(() =>
      expect(apiMock.approveAclPending).toHaveBeenCalledWith(
        [
          {
            channel: "telegram",
            user_id: "pending-1",
            remark: "candidate",
          },
        ],
        "agent-1",
      ),
    );
  });

  it("saves edited pending metadata through remark and username routes", async () => {
    renderDrawer();
    await screen.findByText("pending-1");

    const pendingRow = screen.getByText("pending-1").closest('[role="row"]');
    expect(pendingRow).not.toBeNull();
    const username = within(pendingRow as HTMLElement).getByLabelText(
      "Username",
    );
    const remark = within(pendingRow as HTMLElement).getByLabelText("Remark");
    fireEvent.change(username, { target: { value: "Alice Updated" } });
    fireEvent.change(remark, { target: { value: "approved later" } });

    fireEvent.click(
      within(pendingRow as HTMLElement).getByRole("button", { name: "Save" }),
    );

    await waitFor(() => {
      expect(apiMock.updatePendingRemark).toHaveBeenCalledWith(
        "telegram",
        "pending-1",
        "approved later",
        "agent-1",
      );
      expect(apiMock.updateUsername).toHaveBeenCalledWith(
        "telegram",
        "pending-1",
        "Alice Updated",
        "agent-1",
      );
    });
  });

  it("adds manual whitelist entries and removes selected blacklist entries", async () => {
    renderDrawer();
    await screen.findByText("pending-1");

    fireEvent.change(screen.getByLabelText("Channel"), {
      target: { value: "telegram" },
    });
    fireEvent.change(screen.getByLabelText("User ID"), {
      target: { value: "manual-user" },
    });
    fireEvent.change(screen.getAllByLabelText("Username")[0], {
      target: { value: "Manual" },
    });
    fireEvent.change(screen.getAllByLabelText("Remark")[0], {
      target: { value: "vip" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add whitelist" }));

    await waitFor(() =>
      expect(apiMock.addAclWhitelist).toHaveBeenCalledWith(
        [
          {
            channel: "telegram",
            user_id: "manual-user",
            username: "Manual",
            remark: "vip",
          },
        ],
        "agent-1",
      ),
    );

    fireEvent.click(screen.getByRole("tab", { name: "Blacklist (1)" }));
    await screen.findByText("blocked");
    const blacklistRow = screen.getByText("blocked").closest('[role="row"]');
    expect(blacklistRow).not.toBeNull();
    fireEvent.click(
      within(blacklistRow as HTMLElement).getByLabelText("Select user"),
    );
    fireEvent.click(screen.getByRole("button", { name: "Remove selected" }));

    await waitFor(() =>
      expect(apiMock.removeAclBlacklist).toHaveBeenCalledWith(
        [{ channel: "feishu", user_id: "blocked" }],
        "agent-1",
      ),
    );
  });
});
