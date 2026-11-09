import { useEffect, useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter, useLocation, useNavigate } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import TerminalRuntimeHost from "./TerminalRuntimeHost";

const runtimeMounted = vi.fn();
const runtimeDisposed = vi.fn();

vi.mock("./index", () => ({
  default: function MockTerminalPage({
    routeActive,
  }: {
    routeActive: boolean;
  }) {
    const [screenValue, setScreenValue] = useState(0);
    useEffect(() => {
      runtimeMounted();
      return () => runtimeDisposed();
    }, []);
    return (
      <div>
        <span>screen:{screenValue}</span>
        <span>active:{String(routeActive)}</span>
        <button
          type="button"
          onClick={() => setScreenValue((value) => value + 1)}
        >
          Write terminal
        </button>
      </div>
    );
  },
}));

function Harness() {
  const location = useLocation();
  const navigate = useNavigate();
  return (
    <>
      <button type="button" onClick={() => navigate("/")}>
        Open chat
      </button>
      <button type="button" onClick={() => navigate("/terminal")}>
        Open terminal
      </button>
      <TerminalRuntimeHost active={location.pathname === "/terminal"} />
    </>
  );
}

describe("TerminalRuntimeHost", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("keeps one runtime and observable screen state across route changes", async () => {
    render(
      <MemoryRouter initialEntries={["/terminal"]}>
        <Harness />
      </MemoryRouter>,
    );

    fireEvent.click(
      await screen.findByRole("button", { name: "Write terminal" }),
    );
    expect(screen.getByText("screen:1")).toBeInTheDocument();
    expect(runtimeMounted).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Open chat" }));
    expect(
      document.querySelector("[data-terminal-runtime-host]"),
    ).toHaveAttribute("hidden");
    expect(runtimeDisposed).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Open terminal" }));
    expect(await screen.findByText("screen:1")).toBeInTheDocument();
    expect(screen.getByText("active:true")).toBeInTheDocument();
    expect(document.querySelector("[data-terminal-runtime-host]")).toHaveStyle({
      height: "100%",
      minHeight: "0",
      minWidth: "0",
      overflow: "hidden",
    });
    expect(runtimeMounted).toHaveBeenCalledTimes(1);
    expect(runtimeDisposed).not.toHaveBeenCalled();
  });

  it("disposes the runtime exactly once when the browser shell unmounts", async () => {
    const view = render(
      <MemoryRouter initialEntries={["/terminal"]}>
        <Harness />
      </MemoryRouter>,
    );
    await screen.findByText("screen:0");

    view.unmount();

    expect(runtimeMounted).toHaveBeenCalledTimes(1);
    expect(runtimeDisposed).toHaveBeenCalledTimes(1);
  });
});
