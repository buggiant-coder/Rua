import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import Skeleton from "./index";

function root(container: HTMLElement) {
  return container.firstElementChild as HTMLElement;
}

describe("Skeleton", () => {
  it("renders an accessible list skeleton by default", () => {
    const { container } = render(<Skeleton />);

    expect(root(container)).toHaveAttribute("aria-busy", "true");
    expect(root(container)).toHaveAttribute("aria-live", "polite");
    expect(container.querySelectorAll('[style*="width: 40px"]')).toHaveLength(
      5,
    );
  });

  it("uses variant defaults for card, detail, and table skeletons", () => {
    const { container, rerender } = render(<Skeleton variant="card" />);
    expect(container.querySelectorAll('[style*="width: 32px"]')).toHaveLength(
      4,
    );

    rerender(<Skeleton variant="detail" />);
    expect(container.querySelectorAll('[style*="width: 60%"]')).toHaveLength(1);

    rerender(<Skeleton variant="table" />);
    expect(container.querySelectorAll('[style*="width: 40px"]')).toHaveLength(
      0,
    );
    expect(container.querySelectorAll('[class*="tableRow"]')).toHaveLength(7);
  });

  it("honors explicit count and root styling props", () => {
    const { container } = render(
      <Skeleton
        variant="list"
        count={2}
        className="extra-loading"
        style={{ minHeight: 120 }}
      />,
    );

    expect(root(container)).toHaveClass("extra-loading");
    expect(root(container)).toHaveStyle({ minHeight: "120px" });
    expect(container.querySelectorAll('[style*="width: 40px"]')).toHaveLength(
      2,
    );
  });
});
