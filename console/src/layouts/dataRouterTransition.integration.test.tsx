import { fireEvent, render, screen } from "@testing-library/react";
import {
  Outlet,
  RouterProvider,
  createMemoryRouter,
  useNavigate,
} from "react-router-dom";
import { describe, expect, it } from "vitest";
import { RouteCommitBoundary } from "./AppRouter";

function Layout() {
  return (
    <RouteCommitBoundary fallback={<div>Loading route</div>}>
      <Outlet />
    </RouteCommitBoundary>
  );
}

function CurrentPage() {
  const navigate = useNavigate();
  return (
    <div>
      <span>Current page</span>
      <button type="button" onClick={() => void navigate("/settings")}>
        Open settings
      </button>
    </div>
  );
}

describe("data router page transition", () => {
  it("keeps the current page until the destination loader completes", async () => {
    let finishLoader: (() => void) | undefined;
    const loader = new Promise<void>((resolve) => {
      finishLoader = resolve;
    });
    const router = createMemoryRouter(
      [
        {
          element: <Layout />,
          children: [
            { path: "/current", element: <CurrentPage /> },
            {
              path: "/settings",
              loader: () => loader,
              element: <div>Settings page</div>,
            },
          ],
        },
      ],
      { initialEntries: ["/current"] },
    );

    render(<RouterProvider router={router} useTransitions={false} />);
    fireEvent.click(screen.getByRole("button", { name: "Open settings" }));

    expect(screen.getByText("Current page")).toBeVisible();
    expect(screen.queryByText("Settings page")).not.toBeInTheDocument();

    finishLoader?.();

    expect(await screen.findByText("Settings page")).toBeVisible();
    expect(screen.queryByText("Current page")).not.toBeInTheDocument();
  });
});
