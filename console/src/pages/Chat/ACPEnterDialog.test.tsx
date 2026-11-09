import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ACPEnterDialog from "./ACPEnterDialog";

const api = vi.hoisted(() => ({
  listAcpRunners: vi.fn(),
  listAcpSessions: vi.fn(),
  enterAcp: vi.fn(),
}));
const translation = vi.hoisted(() => ({
  t: (_key: string, fallback: unknown) =>
    typeof fallback === "string" ? fallback : _key,
}));
vi.mock("@/api/modules/conversation", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  conversationApi: api,
}));
vi.mock("react-i18next", () => ({ useTranslation: () => translation }));
vi.mock("@/ui", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  Modal: ({
    open,
    children,
    footer,
  }: {
    open: boolean;
    children: React.ReactNode;
    footer: React.ReactNode;
  }) =>
    open ? (
      <div>
        {children}
        {footer}
      </div>
    ) : null,
}));

describe("ACPEnterDialog first-input selection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.listAcpRunners.mockResolvedValue([
      { key: "codex", enabled: true, name: "Codex" },
    ]);
    api.listAcpSessions.mockResolvedValue([]);
    api.enterAcp.mockResolvedValue({
      ok: true,
      status: { active: true },
      history_preview: null,
    });
  });

  it("stores an unmaterialized selection without entering ACP", async () => {
    const selected = vi.fn();
    const entered = vi.fn();
    render(
      <ACPEnterDialog
        open
        onClose={vi.fn()}
        agentId="default"
        chatId="draft"
        unmaterialized
        onDraftSelected={selected}
        onEntered={entered}
      />,
    );
    const button = await screen.findByRole("button", { name: "新建会话进入" });
    await waitFor(() => expect(button).not.toBeDisabled());
    fireEvent.click(button);
    await waitFor(() =>
      expect(selected).toHaveBeenCalledWith({
        runner: "codex",
        cwd: undefined,
      }),
    );
    expect(api.enterAcp).not.toHaveBeenCalled();
    expect(entered).not.toHaveBeenCalled();
  });

  it("keeps the explicit enter operation for an existing Chat", async () => {
    const entered = vi.fn();
    render(
      <ACPEnterDialog
        open
        onClose={vi.fn()}
        agentId="default"
        chatId="existing"
        onEntered={entered}
      />,
    );
    const button = await screen.findByRole("button", { name: "新建会话进入" });
    await waitFor(() => expect(button).not.toBeDisabled());
    fireEvent.click(button);
    await waitFor(() => expect(api.enterAcp).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(entered).toHaveBeenCalled());
  });

  it("keeps a draft resume choice local without entering a Session", async () => {
    api.listAcpSessions.mockResolvedValue([
      {
        session_id: "resume-1",
        title: "Existing session",
        cwd: "/workspace",
        updated_at: "2026-01-01T00:00:00Z",
      },
    ]);
    const selected = vi.fn();
    render(
      <ACPEnterDialog
        open
        onClose={vi.fn()}
        agentId="default"
        chatId="draft"
        unmaterialized
        onDraftSelected={selected}
        onEntered={vi.fn()}
      />,
    );
    const button = await screen.findByRole("button", { name: "Resume" });
    fireEvent.click(button);
    await waitFor(() =>
      expect(selected).toHaveBeenCalledWith({
        runner: "codex",
        cwd: undefined,
        resume_session_id: "resume-1",
      }),
    );
    expect(api.enterAcp).not.toHaveBeenCalled();
  });
});
