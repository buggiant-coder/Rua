import type { NavigateFunction, NavigateOptions, To } from "react-router-dom";

export function navigateWithPageTransition(
  navigate: NavigateFunction,
  to: To,
  options?: NavigateOptions,
) {
  if (options) {
    void navigate(to, options);
  } else {
    void navigate(to);
  }
}

export function runWithPageTransition(update: () => void) {
  update();
}
