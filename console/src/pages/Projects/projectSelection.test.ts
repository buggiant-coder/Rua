import { beforeEach, describe, expect, it } from "vitest";

import {
  forgetRememberedProject,
  lastProjectId,
  rememberProjectId,
} from "./projectSelection";

describe("projectSelection", () => {
  beforeEach(() => localStorage.clear());

  it("shares the most recently selected Project across surfaces", () => {
    expect(lastProjectId()).toBeNull();
    rememberProjectId("project-b");
    expect(lastProjectId()).toBe("project-b");
    forgetRememberedProject("project-a");
    expect(lastProjectId()).toBe("project-b");
    forgetRememberedProject("project-b");
    expect(lastProjectId()).toBeNull();
  });
});
