import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { it, expect, vi } from "vitest";
import Dropdown from ".";
it("supports keyboard menu navigation, activation and focus return", async () => {
  const selected = vi.fn();
  render(
    <Dropdown
      items={[
        { key: "first", label: "First" },
        { key: "disabled", label: "Disabled", disabled: true },
        { key: "last", label: "Last", onClick: selected },
      ]}
    >
      <button>Actions</button>
    </Dropdown>,
  );
  const trigger = screen.getByRole("button", { name: "Actions" });
  trigger.focus();
  fireEvent.click(trigger);
  const first = await screen.findByRole("menuitem", { name: "First" });
  await waitFor(() => expect(first).toHaveFocus());
  fireEvent.keyDown(first, { key: "ArrowDown" });
  expect(screen.getByRole("menuitem", { name: "Last" })).toHaveFocus();
  fireEvent.keyDown(document.activeElement!, { key: "Enter" });
  expect(selected).toHaveBeenCalledOnce();
  await waitFor(() => expect(trigger).toHaveFocus());
});
