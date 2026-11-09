import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { FilterBar } from "./FilterBar";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "sessions.searchTitle": "Search title",
        "sessions.sortByChannel": "Channel",
        "sessions.sortByName": "Name",
        "sessions.sortByUserId": "User",
        "sessions.sortDefault": "Default",
      })[key] ?? key,
  }),
}));

vi.mock("@/ui", () => ({
  Btn: ({ children }: { children?: React.ReactNode }) => (
    <button type="button">{children ?? "sort"}</button>
  ),
  Dropdown: ({
    children,
    menu,
  }: {
    children: React.ReactNode;
    menu?: {
      items?: Array<{ key: string; label: string }>;
      onClick?: (event: { key: string }) => void;
    };
  }) => (
    <div>
      {children}
      {menu?.items?.map((item) => (
        <button
          key={item.key}
          type="button"
          onClick={() => menu.onClick?.({ key: item.key })}
        >
          {item.label}
        </button>
      ))}
    </div>
  ),
  TextInput: ({
    onChange,
    placeholder,
    value,
  }: {
    onChange?: (event: React.ChangeEvent<HTMLInputElement>) => void;
    placeholder?: string;
    value?: string;
  }) => (
    <input placeholder={placeholder} value={value ?? ""} onChange={onChange} />
  ),
}));

describe("Sessions FilterBar", () => {
  it("updates Rust session title search text", () => {
    const onSearchTitleChange = vi.fn();

    render(
      <FilterBar
        searchTitle=""
        sortField="default"
        sortOrder="desc"
        onSearchTitleChange={onSearchTitleChange}
        onSortFieldChange={vi.fn()}
        onSortOrderChange={vi.fn()}
      />,
    );

    fireEvent.change(screen.getByPlaceholderText("Search title"), {
      target: { value: "release" },
    });

    expect(onSearchTitleChange).toHaveBeenCalledWith("release");
  });

  it("splits sort menu keys into field and order", () => {
    const onSortFieldChange = vi.fn();
    const onSortOrderChange = vi.fn();

    render(
      <FilterBar
        searchTitle=""
        sortField="default"
        sortOrder="desc"
        onSearchTitleChange={vi.fn()}
        onSortFieldChange={onSortFieldChange}
        onSortOrderChange={onSortOrderChange}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "User Z→A" }));

    expect(onSortFieldChange).toHaveBeenCalledWith("user_id");
    expect(onSortOrderChange).toHaveBeenCalledWith("desc");
  });
});
