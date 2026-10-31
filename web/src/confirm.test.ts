import { describe, expect, test } from "vitest";
import { CONFIRM_WINDOW_MS, expire, press, type ConfirmState } from "./confirm";

const idle: ConfirmState = { armedAt: null };

describe("confirm-by-second-click", () => {
  test("the first press arms without firing", () => {
    const r = press(idle, 1000);
    expect(r.fire).toBe(false);
    expect(r.state.armedAt).toBe(1000);
  });

  test("a second press inside the window fires and disarms", () => {
    const armed = press(idle, 1000).state;
    const r = press(armed, 1000 + CONFIRM_WINDOW_MS);
    expect(r.fire).toBe(true);
    expect(r.state.armedAt).toBeNull();
  });

  test("a second press after the window re-arms instead of firing", () => {
    const armed = press(idle, 1000).state;
    const r = press(armed, 1000 + CONFIRM_WINDOW_MS + 1);
    expect(r.fire).toBe(false);
    expect(r.state.armedAt).toBe(1000 + CONFIRM_WINDOW_MS + 1);
  });

  test("expire disarms only once the window has passed", () => {
    const armed = press(idle, 1000).state;
    expect(expire(armed, 2500).armedAt).toBe(1000);
    expect(expire(armed, 3001).armedAt).toBeNull();
  });
});
