import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { RenderStateRoot, RenderStateScope } from "./viewState";
import { useRenderState, type RenderViewState } from "./useRenderState";

it("restores a control after its turn unmounts and keeps another turn independent", () => {
  const store: RenderViewState = {
    values: new Map(),
    protectedScopes: new Map(),
  };
  function Control() {
    const [open, setOpen] = useRenderState("details", false, false);
    return (
      <button onClick={() => setOpen(!open)}>
        {open ? "Expanded" : "Collapsed"}
      </button>
    );
  }
  const tree = (turn: string | null) => (
    <RenderStateRoot store={store}>
      {turn && (
        <RenderStateScope key={turn} name={turn}>
          <Control />
        </RenderStateScope>
      )}
    </RenderStateRoot>
  );
  const { rerender } = render(tree("first"));
  fireEvent.click(screen.getByRole("button"));
  rerender(tree(null));
  rerender(tree("second"));
  expect(screen.getByRole("button")).toHaveTextContent("Collapsed");
  rerender(tree("first"));
  expect(screen.getByRole("button")).toHaveTextContent("Expanded");
});
