import { fireEvent, render, screen } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import type { PrinterView, StepperView } from "@/api";
import { GuidedSetup } from "./GuidedSetup";

vi.mock("@/api", async (orig) => ({ ...(await orig<typeof import("@/api")>()), postTune: vi.fn() }));

const stepper = (name: string, axis: string): StepperView =>
  ({
    name,
    mcu: "m",
    channel: 0,
    serial: "s",
    kinematics: "corexy",
    motor_rotation_distance_mm: 40,
    axis,
    position_min: 0,
    position_max: 300,
    encoder_ppr: 4096,
    electrical_model_present: true,
    config_gains: {
      position_p: 800,
      position_i: 0,
      velocity_p: 1000,
      velocity_i: 50,
      velocity_filter_hz: 500,
      torque_filter_hz: 1000,
      position_filter_hz: 500,
      flux_filter_hz: 1000,
    },
    unusable: null,
    proposal: { pattern: { start: [50, 50], end: [250, 250] }, refusal: null },
  }) as StepperView;

const printer = {
  steppers: [stepper("stepper_x", "x"), stepper("stepper_y", "y")],
  preconditions: [],
  moonraker: { max_velocity: 500 },
} as unknown as PrinterView;

test("the standstill factor is editable only while the standstill gate is on", () => {
  render(<GuidedSetup printer={printer} onCreated={() => undefined} />);
  expect(screen.getByRole("spinbutton", { name: "Standstill factor" })).toBeDisabled();
  fireEvent.click(screen.getByRole("checkbox", { name: "Standstill gate" }));
  expect(screen.getByRole("spinbutton", { name: "Standstill factor" })).toBeEnabled();
});
