import { fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import EmptyState from "./index";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, fallback?: string) =>
      ({
        "common.refresh": "Refresh",
        "emptyState.empty": "No data",
        "emptyState.error": "Load failed",
        "emptyState.forbidden": "Forbidden",
        "emptyState.noResults": "No results",
        "emptyState.notAvailable": "Not available",
      })[key] ??
      fallback ??
      key,
  }),
}));

vi.mock("lucide-react", () => ({
  AlertCircle: () => <span data-testid="icon-error" />,
  Inbox: () => <span data-testid="icon-empty" />,
  Lock: () => <span data-testid="icon-forbidden" />,
  Plug: () => <span data-testid="icon-not-available" />,
  RefreshCw: () => <span data-testid="icon-refresh" />,
  Search: () => <span data-testid="icon-no-results" />,
}));

vi.mock("@/ui", () => ({
  Btn: ({
    children,
    icon,
    onClick,
  }: {
    children?: ReactNode;
    icon?: ReactNode;
    onClick?: () => void;
  }) => (
    <button type="button" onClick={onClick}>
      {icon}
      {children}
    </button>
  ),
}));

describe("EmptyState", () => {
  it("renders Rust default empty copy and icon", () => {
    render(<EmptyState />);

    expect(screen.getByTestId("icon-empty")).toBeVisible();
    expect(screen.getByText("No data")).toBeVisible();
  });

  it("maps variants to their default descriptions and icons", () => {
    const { rerender } = render(<EmptyState variant="no-results" />);
    expect(screen.getByTestId("icon-no-results")).toBeVisible();
    expect(screen.getByText("No results")).toBeVisible();

    rerender(<EmptyState variant="not-available" />);
    expect(screen.getByTestId("icon-not-available")).toBeVisible();
    expect(screen.getByText("Not available")).toBeVisible();

    rerender(<EmptyState variant="forbidden" />);
    expect(screen.getByTestId("icon-forbidden")).toBeVisible();
    expect(screen.getByText("Forbidden")).toBeVisible();
  });

  it("renders a default retry action only for error states with onRetry", () => {
    const onRetry = vi.fn();

    render(<EmptyState variant="error" onRetry={onRetry} />);

    expect(screen.getByTestId("icon-error")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: /Refresh/ }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("prefers custom content and hides actions in inline mode", () => {
    const action = <button type="button">Create</button>;
    const { rerender } = render(
      <EmptyState
        icon={<span data-testid="custom-icon" />}
        description="Custom empty"
        action={action}
      />,
    );

    expect(screen.getByTestId("custom-icon")).toBeVisible();
    expect(screen.getByText("Custom empty")).toBeVisible();
    expect(screen.getByRole("button", { name: "Create" })).toBeVisible();

    rerender(
      <EmptyState
        size="inline"
        description="Inline empty"
        action={action}
        onRetry={vi.fn()}
      />,
    );

    expect(screen.getByText("Inline empty")).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "Create" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /Refresh/ }),
    ).not.toBeInTheDocument();
  });
});
