import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { MergedRule } from "../useToolGuard";
import { RuleTable } from "./RuleTable";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: { defaultValue?: string } | string) => {
      const labels: Record<string, string> = {
        "security.rules.searchRule": "Search rules",
        "security.rules.sortById": "Sort by id",
        "security.rules.sortByEnabled": "Sort by enabled",
        "security.rules.emptyText": "No rules",
        "security.rules.builtin": "Builtin",
        "security.rules.custom": "Custom",
        "security.rules.categories.command_injection": "Command injection",
        "security.rules.categories.sensitive_file_access":
          "Sensitive file access",
      };
      if (key.startsWith("security.rules.descriptions.")) {
        return typeof options === "object" ? options.defaultValue ?? "" : "";
      }
      return labels[key] ?? (typeof options === "string" ? options : key);
    },
  }),
}));

vi.mock("../../../../contexts/ThemeContext", () => ({
  useTheme: () => ({ isDark: false }),
}));

vi.mock("../../../../utils/isMobile", () => ({
  isMobile: () => false,
}));

const rules: MergedRule[] = [
  {
    id: "bash-danger",
    tools: ["Bash"],
    params: ["command"],
    category: "command_injection",
    severity: "HIGH",
    patterns: ["rm -rf"],
    exclude_patterns: [],
    description: "Blocks destructive shell commands",
    remediation: "Review the command",
    source: "builtin",
    disabled: false,
  },
  {
    id: "read-secret",
    tools: ["Read"],
    params: ["path"],
    category: "sensitive_file_access",
    severity: "CRITICAL",
    patterns: ["~/.ssh"],
    exclude_patterns: [],
    description: "Blocks secret file reads",
    remediation: "Use an allowed path",
    source: "custom",
    disabled: true,
  },
];

describe("RuleTable", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("groups and filters Rust canonical tool-guard rules", () => {
    render(
      <RuleTable
        rules={rules}
        enabled
        onToggleRule={vi.fn()}
        onPreviewRule={vi.fn()}
        onEditRule={vi.fn()}
        onDeleteRule={vi.fn()}
      />,
    );

    expect(screen.getByText("Command injection")).toBeVisible();
    expect(screen.getByText("Sensitive file access")).toBeVisible();
    expect(screen.getByText("1/1")).toBeVisible();
    expect(screen.getByText("0/1")).toBeVisible();
    expect(screen.getByText("bash-danger")).toBeVisible();
    expect(screen.getByText("read-secret")).toBeVisible();
    expect(screen.getByText("Blocks secret file reads")).toBeVisible();

    fireEvent.change(screen.getByPlaceholderText("Search rules"), {
      target: { value: "read" },
    });

    expect(screen.queryByText("bash-danger")).not.toBeInTheDocument();
    expect(screen.getByText("read-secret")).toBeVisible();
  });

  it("dispatches preview, toggle, edit, delete, and add callbacks", () => {
    const onPreviewRule = vi.fn();
    const onToggleRule = vi.fn();
    const onEditRule = vi.fn();
    const onDeleteRule = vi.fn();
    const onAddRule = vi.fn();

    render(
      <RuleTable
        rules={rules}
        enabled
        onToggleRule={onToggleRule}
        onPreviewRule={onPreviewRule}
        onEditRule={onEditRule}
        onDeleteRule={onDeleteRule}
        onAddRule={onAddRule}
      />,
    );

    fireEvent.click(screen.getByText("bash-danger"));
    expect(onPreviewRule).toHaveBeenCalledWith(rules[0]);

    const readRow = screen.getByText("read-secret").closest('[role="row"]');
    expect(readRow).not.toBeNull();
    const row = within(readRow as HTMLElement);

    fireEvent.click(row.getByRole("checkbox"));
    expect(onToggleRule).toHaveBeenCalledWith("read-secret", true);

    const actionButtons = row.getAllByRole("button");
    fireEvent.click(actionButtons[0]);
    fireEvent.click(actionButtons[1]);
    expect(onEditRule).toHaveBeenCalledWith(rules[1]);
    expect(onDeleteRule).toHaveBeenCalledWith("read-secret");

    fireEvent.click(screen.getAllByRole("button")[1]);
    expect(onAddRule).toHaveBeenCalledTimes(1);
  });

  it("disables rule actions when the Tool Guard profile is disabled", () => {
    const onToggleRule = vi.fn();
    const onEditRule = vi.fn();
    const onDeleteRule = vi.fn();
    const onAddRule = vi.fn();

    render(
      <RuleTable
        rules={rules}
        enabled={false}
        onToggleRule={onToggleRule}
        onPreviewRule={vi.fn()}
        onEditRule={onEditRule}
        onDeleteRule={onDeleteRule}
        onAddRule={onAddRule}
        addRuleDisabled
      />,
    );

    const readRow = screen.getByText("read-secret").closest('[role="row"]');
    expect(readRow).not.toBeNull();
    const row = within(readRow as HTMLElement);

    expect(row.getByRole("checkbox")).toBeDisabled();
    for (const button of row.getAllByRole("button")) {
      expect(button).toBeDisabled();
    }
    expect(screen.getAllByRole("button")[1]).toBeDisabled();
  });
});
