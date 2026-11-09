import { describe, expect, it } from "vitest";
import en from "./en.json";
import ja from "./ja.json";
import ru from "./ru.json";
import zh from "./zh.json";

const locales = { en, ja, ru, zh };

function leafPaths(value: Record<string, unknown>, prefix = ""): string[] {
  return Object.entries(value).flatMap(([key, child]) => {
    const path = prefix ? `${prefix}.${key}` : key;
    return child && typeof child === "object"
      ? leafPaths(child as Record<string, unknown>, path)
      : [path];
  });
}

function valueAtPath(value: Record<string, unknown>, path: string): unknown {
  return path
    .split(".")
    .reduce<unknown>(
      (current, segment) =>
        current && typeof current === "object"
          ? (current as Record<string, unknown>)[segment]
          : undefined,
      value,
    );
}

describe("Project Chat translations", () => {
  const requiredPaths = leafPaths({
    chat: en.projects.chat,
    workspace: en.projects.workspace,
  });

  it.each(Object.entries(locales))(
    "%s defines every Project Chat and Workspace label",
    (_locale, messages) => {
      for (const path of requiredPaths) {
        expect(
          valueAtPath(
            messages.projects as unknown as Record<string, unknown>,
            path,
          ),
        ).toBeTruthy();
      }
    },
  );

  it("uses Chinese labels for the Project Chat controls", () => {
    expect(zh.projects.chat.title).toBe("项目对话");
    expect(zh.projects.chat.model).toBe("项目对话模型");
    expect(zh.projects.chat.agent).toBe("项目对话智能体");
    expect(zh.projects.workspace.title).toBe("工作区");
    expect(zh.projects.moreActions).toBe("更多项目操作");
  });
});
