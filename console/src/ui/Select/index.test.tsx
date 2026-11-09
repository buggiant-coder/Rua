import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it } from "vitest";
import Select from ".";

function ControlledSearchSelect() {
  const [search, setSearch] = useState("");
  const [value, setValue] = useState<string | undefined>(undefined);
  return (
    <Select
      showSearch
      value={value}
      searchValue={search}
      onSearch={setSearch}
      onChange={(next) => setValue(typeof next === "string" ? next : undefined)}
      placeholder="/default/workspace"
      options={[
        { value: "/default/workspace", label: "/default/workspace" },
        { value: "~/MyProjects/rua-rust", label: "~/MyProjects/rua-rust" },
      ]}
    />
  );
}

describe("Select", () => {
  it("uses the shared Lucide chevron and rotates it while open", () => {
    const { container } = render(
      <Select
        aria-label="Example"
        value="one"
        options={[
          { value: "one", label: "One" },
          { value: "two", label: "Two" },
        ]}
      />,
    );

    const arrow = container.querySelector(".rua-ui-select-arrow");
    expect(arrow?.tagName.toLowerCase()).toBe("svg");

    fireEvent.click(screen.getByRole("combobox", { name: "Example" }));

    expect(arrow).toHaveClass("rua-ui-select-arrow--open");
  });

  it("supports controlled searchable single select input", () => {
    render(<ControlledSearchSelect />);

    const input = screen.getByPlaceholderText("/default/workspace");
    fireEvent.focus(input);

    expect(screen.getByText("/default/workspace")).toBeInTheDocument();
    expect(screen.getByText("~/MyProjects/rua-rust")).toBeInTheDocument();

    fireEvent.change(input, { target: { value: "rua" } });

    expect(screen.queryByText("/default/workspace")).not.toBeInTheDocument();
    expect(screen.getByText("~/MyProjects/rua-rust")).toBeInTheDocument();
  });
});
