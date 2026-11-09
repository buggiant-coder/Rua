import { describe, expect, it } from "vitest";
import { followAfterScroll } from "../viewportFollow";

describe("viewport follow ownership", () => {
  it.each([0, 0.5, 4, 8, 79, 200])(
    "upward movement releases following at distance %s",
    (distanceFromBottom) => {
      expect(
        followAfterScroll({
          following: true,
          direction: -1,
          distanceFromBottom,
          programmatic: false,
        }),
      ).toBe(false);
    },
  );

  it("only downward arrival restores following", () => {
    expect(
      followAfterScroll({
        following: false,
        direction: 1,
        distanceFromBottom: 4,
        programmatic: false,
      }),
    ).toBe(false);
    expect(
      followAfterScroll({
        following: false,
        direction: 1,
        distanceFromBottom: 0.5,
        programmatic: false,
      }),
    ).toBe(true);
  });

  it.each([true, false])(
    "layout and controller scroll preserve following=%s",
    (following) => {
      expect(
        followAfterScroll({
          following,
          direction: 0,
          distanceFromBottom: 0,
          programmatic: false,
        }),
      ).toBe(following);
      expect(
        followAfterScroll({
          following,
          direction: 1,
          distanceFromBottom: 0,
          programmatic: true,
        }),
      ).toBe(following);
      expect(
        followAfterScroll({
          following,
          direction: -1,
          distanceFromBottom: 100,
          programmatic: true,
        }),
      ).toBe(following);
    },
  );
});
