import { describe, expect, test } from "vitest";
import { ApiRequestError } from "./api";
import { failureMessage } from "./failure";

const api = (status: number, message: string, field: string | null = null) =>
  new ApiRequestError(status, message, field);

describe("failureMessage", () => {
  test("an unreachable service says so and what to do", () => {
    expect(failureMessage(api(0, "Failed to fetch"))).toBe(
      "Can't reach foci-tune. Check that the service is running, then try again.",
    );
  });

  test("a timeout is told apart from an unreachable service", () => {
    expect(failureMessage(api(0, "request timed out"))).toBe("foci-tune didn't answer in time. Try again.");
  });

  test.each([
    ["speeds_mm_s", "Test speeds"],
    ["probe.speed_mm_s", "Probe speed"],
    ["probe.segments_between_toggles", "Moves between gain changes"],
    ["gates.marginal", "Low damping below"],
    ["gates.standstill_factor", "Noise limit"],
    ["seed", "Seed"],
  ])("a rejected %s is named as %s on screen", (field, label) => {
    expect(failureMessage(api(400, "must be above zero", field))).toBe(`${label}: must be above zero`);
  });

  test("a field the page has no label for keeps its name", () => {
    expect(failureMessage(api(400, "bad", "mystery"))).toBe("mystery: bad");
  });

  test.each([
    ["the printer is not fully homed", /isn't fully homed.*Home X, Y and Z/],
    ["the printer is printing", /is printing.*try again/],
    ["klipper is not ready", /Klipper isn't ready/],
    ["a run is in flight", /Another run is still in progress/],
    [
      "the stepper is locked until its starting gains are restored: abandon in progress",
      /locked until its starting gains are restored/,
    ],
    ["only 120 bytes free on the data directory", /almost out of storage/],
    [
      "the last emergency stop is unconfirmed and motion may continue; confirm the stop or restart Klipper",
      /emergency stop wasn't confirmed/,
    ],
  ])("the refusal %j reads as a sentence with a next step", (reason, pattern) => {
    expect(failureMessage(api(409, reason))).toMatch(pattern);
  });

  test("a reason the page does not know is shown as the server wrote it", () => {
    expect(failureMessage(api(409, "something new"))).toBe("something new");
  });

  test("other errors fall back to their message or a plain sentence", () => {
    expect(failureMessage(new Error("boom"))).toBe("boom");
    expect(failureMessage("weird")).toBe("Something went wrong. Try again.");
  });
});
