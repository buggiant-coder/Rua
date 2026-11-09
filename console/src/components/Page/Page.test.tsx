import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Page, PageBody } from "./index";

describe("Page", () => {
  it("renders routed page content with caller class names", () => {
    const { container } = render(
      <Page className="custom-page">
        <h1>Settings</h1>
      </Page>,
    );

    expect(screen.getByRole("heading", { name: "Settings" })).toBeVisible();
    expect(container.firstElementChild).toHaveClass("custom-page");
  });

  it("renders page body content with caller class names", () => {
    const { container } = render(
      <PageBody className="custom-body">
        <p>Body content</p>
      </PageBody>,
    );

    expect(screen.getByText("Body content")).toBeVisible();
    expect(container.firstElementChild).toHaveClass("custom-body");
  });

  it("composes page and page body for the standard Rust route shell", () => {
    const { container } = render(
      <Page className="outer">
        <PageBody className="inner">
          <button type="button">Save</button>
        </PageBody>
      </Page>,
    );

    expect(screen.getByRole("button", { name: "Save" })).toBeVisible();
    expect(container.querySelector(".outer")).toContainElement(
      container.querySelector(".inner"),
    );
  });
});
