import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import Popover from ".";

describe("Popover", () => {
  it("applies overlayClassName to the floating panel", async () => {
    render(
      <Popover content={<div>panel</div>} overlayClassName="custom-popover">
        <button type="button">trigger</button>
      </Popover>,
    );

    fireEvent.click(screen.getByRole("button", { name: "trigger" }));

    await waitFor(() => {
      const panel = document.body.querySelector(".custom-popover");
      expect(panel).toBeInTheDocument();
      expect(panel).toHaveClass("rua-ui-popover");
    });
  });

  it("uses the shared single-line surface modifier", async () => {
    render(
      <Popover content="panel" surface="single">
        <button type="button">trigger</button>
      </Popover>,
    );

    fireEvent.click(screen.getByRole("button", { name: "trigger" }));

    await waitFor(() => {
      expect(document.body.querySelector(".rua-ui-popover")).toHaveClass(
        "rua-ui-popover--single",
      );
    });
  });

  it("supports hover and click triggers together", async () => {
    render(
      <Popover content="hover panel" trigger={["hover", "click"]}>
        <button type="button">usage</button>
      </Popover>,
    );

    fireEvent.mouseEnter(
      screen.getByRole("button", { name: "usage" }).parentElement!,
    );

    expect(await screen.findByText("hover panel")).toBeInTheDocument();
  });
});
