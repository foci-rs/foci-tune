import { fireEvent, render, screen, within } from "@testing-library/react";
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
  expect(screen.getByRole("button", { name: "Next" })).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "Fix the path" }));
  expect(screen.queryByRole("button", { name: "Fix the path" })).toBeNull();
  expect(screen.getByRole("button", { name: "Next" })).toBeEnabled();
});

test("the path starts on the default and offers a custom one", () => {
  render(<GuidedSetup printer={printer} onCreated={() => undefined} />);
  expect(screen.getByRole("combobox", { name: "Path" })).toHaveTextContent("Default");
});

test("the test speeds start at 100, 200 and 400 mm/s", () => {
  render(<GuidedSetup printer={printer} onCreated={() => undefined} />);
  expect(screen.getByRole("textbox", { name: "Test speeds (mm/s)" })).toHaveValue("100, 200, 400");
});

test("the noise check switch sits in the same input group as the noise limit, before it", () => {
  render(<GuidedSetup printer={printer} onCreated={() => undefined} />);
  const group = screen
    .getByRole("spinbutton", { name: "Noise limit (times the baseline)" })
    .closest("[data-slot=input-group]") as HTMLElement;
  expect(group).not.toBeNull();
  const toggle = within(group).getByRole("switch", { name: "Check noise at standstill" });
  const input = within(group).getByRole("spinbutton");
  expect(toggle.compareDocumentPosition(input) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
});

test("the low-damping switch says what it changes and why it starts off", () => {
  render(<GuidedSetup printer={printer} onCreated={() => undefined} />);
  const toggle = screen.getByRole("switch", { name: "Include low-damping gains" });
  expect(toggle).not.toBeChecked();
  expect(screen.getByText(/never recommended/i)).toBeInTheDocument();
  expect(screen.getByText(/close to instability/i)).toBeInTheDocument();
});

test("the step-response warning belongs to the probe settings, not the path card", () => {
  render(<GuidedSetup printer={printer} onCreated={() => undefined} />);
  fireEvent.change(screen.getByRole("spinbutton", { name: "Passes" }), { target: { value: "2" } });
  const hint = screen.getByText(/step responses/i);
  expect(hint.closest("section")).toHaveAccessibleName("Tune setup");
  expect(within(screen.getByRole("region", { name: "Path" })).queryByText(/step responses/i)).toBeNull();
});

test("the gain-change spacing warning sits with the probe settings too", () => {
  render(<GuidedSetup printer={printer} onCreated={() => undefined} />);
  fireEvent.change(screen.getByRole("spinbutton", { name: "Speed (mm/s)" }), { target: { value: "400" } });
  fireEvent.change(screen.getByRole("spinbutton", { name: "Moves between gain changes" }), { target: { value: "3" } });
  const note = screen.getByText(/apart; the fit needs/i);
  expect(note.closest("section")).toHaveAccessibleName("Tune setup");
});
