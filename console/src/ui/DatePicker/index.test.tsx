import { fireEvent, render, screen } from "@testing-library/react";
import dayjs from "dayjs";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "@/i18n";
import DatePicker from "./index";

describe("DatePicker.RangePicker", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("shows the selected range and emits a complete updated range", () => {
    const onChange = vi.fn();

    render(
      <DatePicker.RangePicker
        value={[dayjs("2026-05-19"), dayjs("2026-06-18")]}
        onChange={onChange}
      />,
    );

    expect(screen.getByLabelText("Start date")).toHaveValue("2026-05-19");
    expect(screen.getByLabelText("End date")).toHaveValue("2026-06-18");

    fireEvent.click(screen.getByLabelText("Start date"));
    fireEvent.click(screen.getByRole("button", { name: "May 18, 2026" }));

    expect(onChange).toHaveBeenCalledTimes(1);
    const [range] = onChange.mock.calls[0] as [[dayjs.Dayjs, dayjs.Dayjs]];
    expect(range[0].format("YYYY-MM-DD")).toBe("2026-05-18");
    expect(range[1].format("YYYY-MM-DD")).toBe("2026-06-18");
  });

  it("localizes the range controls and calendar for Chinese", async () => {
    await i18n.changeLanguage("zh");

    render(
      <DatePicker.RangePicker
        value={[dayjs("2026-05-19"), dayjs("2026-06-18")]}
      />,
    );

    expect(screen.getByLabelText("开始日期")).toHaveValue("2026-05-19");
    expect(screen.getByLabelText("结束日期")).toHaveValue("2026-06-18");

    fireEvent.click(screen.getByLabelText("开始日期"));

    expect(screen.getByRole("dialog", { name: "选择开始日期" })).toBeVisible();
    expect(screen.getByText("2026年5月")).toBeInTheDocument();
    expect(screen.getByText("周日")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "2026年5月18日" }),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("上个月")).toBeInTheDocument();
    expect(screen.getByLabelText("下个月")).toBeInTheDocument();
  });
});
