import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../../../../api";
import { SkillUpdateButton } from "./SkillUpdateButton";

vi.mock("../../../../api", () => ({
  api: {
    checkPoolSkillUpdate: vi.fn(),
    checkSkillUpdate: vi.fn(),
    updatePoolSkill: vi.fn(),
    updateSkill: vi.fn(),
  },
}));

const mockedApi = vi.mocked(api);

describe("SkillUpdateButton", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("uses pool update APIs when rendered for a pool skill", async () => {
    mockedApi.checkPoolSkillUpdate.mockResolvedValue({
      has_update: true,
      current_version: "1.0.0",
      latest_version: "1.1.0",
      local_modified: false,
    });
    mockedApi.updatePoolSkill.mockResolvedValue({
      updated: true,
      old_version: "1.0.0",
      new_version: "1.1.0",
    });
    const onUpdated = vi.fn();

    render(
      <SkillUpdateButton
        skillName="writer"
        scope="pool"
        onUpdated={onUpdated}
      />,
    );

    fireEvent.click(screen.getAllByRole("button")[0]);
    await waitFor(() => {
      expect(mockedApi.checkPoolSkillUpdate).toHaveBeenCalledWith("writer");
    });
    expect(mockedApi.checkSkillUpdate).not.toHaveBeenCalled();

    fireEvent.click(screen.getAllByRole("button")[1]);
    await waitFor(() => {
      expect(mockedApi.updatePoolSkill).toHaveBeenCalledWith("writer", false);
    });
    expect(mockedApi.updateSkill).not.toHaveBeenCalled();
    expect(onUpdated).toHaveBeenCalled();
  });
});
