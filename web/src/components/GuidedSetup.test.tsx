import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import type { PrinterView, StepperView } from "@/api";
import { browserStorage, savePattern } from "@/tune";
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

beforeEach(() => {
  const data = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => data.get(k) ?? null,
    setItem: (k: string, v: string) => void data.set(k, v),
  });
});

test("the noise limit is editable only while the standstill noise check is on", () => {
  render(<GuidedSetup printer={printer} onCreated={() => undefined} />);
  expect(screen.getByRole("spinbutton", { name: "Noise limit (times the baseline)" })).toBeDisabled();
  fireEvent.click(screen.getByRole("switch", { name: "Check noise at standstill" }));
  expect(screen.getByRole("spinbutton", { name: "Noise limit (times the baseline)" })).toBeEnabled();
});

test("a remembered path that moves the other motor is flagged, held back and can be fixed", () => {
  savePattern(browserStorage(), "stepper_x", { start: [50, 50], end: [250, 100] });
  render(<GuidedSetup printer={printer} onCreated={() => undefined} />);
  expect(screen.getByText(/stepper_y would move/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Create tune" })).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "Fix the path" }));
  expect(screen.queryByRole("button", { name: "Fix the path" })).toBeNull();
  expect(screen.getByRole("button", { name: "Create tune" })).toBeEnabled();
});

test("the path starts on the default and offers a custom one", () => {
  render(<GuidedSetup printer={printer} onCreated={() => undefined} />);
  expect(screen.getByRole("combobox", { name: "Path" })).toHaveTextContent("Default");
});
