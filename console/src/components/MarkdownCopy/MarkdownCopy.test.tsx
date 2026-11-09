import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import type React from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

const { mockCopyText, mockToastSuccess, mockToastError } = vi.hoisted(() => ({
  mockCopyText: vi.fn(),
  mockToastSuccess: vi.fn(),
  mockToastError: vi.fn(),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

vi.mock("../RuaMarkdown", () => ({
  RuaMarkdown: ({ content }: { content: string }) => (
    <article data-testid="markdown-preview">{content}</article>
  ),
}));

vi.mock("../../utils/clipboard", () => ({
  copyText: mockCopyText,
}));

vi.mock("@/ui", () => {
  return {
    Btn: ({
      children,
      icon,
      onClick,
      loading,
    }: {
      children?: React.ReactNode;
      icon?: React.ReactNode;
      onClick?: () => void;
      loading?: boolean;
    }) => (
      <button type="button" disabled={loading} onClick={onClick}>
        {icon}
        {children}
      </button>
    ),
    TextArea: (props: React.TextareaHTMLAttributes<HTMLTextAreaElement>) => (
      <textarea {...props} />
    ),
    Toggle: ({
      checked,
      onChange,
    }: {
      checked?: boolean;
      onChange?: React.ChangeEventHandler<HTMLInputElement>;
    }) => <input type="checkbox" checked={checked} onChange={onChange} />,
    toast: {
      success: mockToastSuccess,
      error: mockToastError,
    },
  };
});

import { MarkdownCopy } from "./MarkdownCopy";

describe("MarkdownCopy", () => {
  afterEach(() => vi.clearAllMocks());

  it("strips frontmatter in preview while copying the original markdown content", async () => {
    mockCopyText.mockResolvedValueOnce(true);
    const content = "---\ntitle: Test\n---\n# Body";

    render(<MarkdownCopy content={content} />);

    expect(screen.getByTestId("markdown-preview")).toHaveTextContent("# Body");
    expect(screen.getByTestId("markdown-preview")).not.toHaveTextContent(
      "title: Test",
    );

    fireEvent.click(screen.getByRole("button"));

    expect(mockCopyText).toHaveBeenCalledWith(content);
    await waitFor(() =>
      expect(mockToastSuccess).toHaveBeenCalledWith("common.copied"),
    );
  });

  it("switches to editable text mode and copies edited content", async () => {
    mockCopyText.mockResolvedValueOnce(true);
    const onContentChange = vi.fn();

    render(
      <MarkdownCopy
        content="initial"
        editable
        onContentChange={onContentChange}
      />,
    );

    const textarea = screen.getByRole("textbox");
    fireEvent.change(textarea, { target: { value: "edited" } });
    fireEvent.click(screen.getByRole("button"));

    expect(onContentChange).toHaveBeenLastCalledWith("edited");
    expect(mockCopyText).toHaveBeenCalledWith("edited");
  });

  it("shows copy failure feedback", async () => {
    mockCopyText.mockResolvedValueOnce(false);

    render(<MarkdownCopy content="copy me" />);
    fireEvent.click(screen.getByRole("button"));

    await waitFor(() =>
      expect(mockToastError).toHaveBeenCalledWith("common.copyFailed"),
    );
  });

  it("can hide only the copy button while keeping preview controls", () => {
    render(<MarkdownCopy content="copy me" showCopyButton={false} />);

    expect(screen.getByText("common.preview")).toBeVisible();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});
