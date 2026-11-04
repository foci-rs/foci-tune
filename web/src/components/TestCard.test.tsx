import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import type { StepperView } from "@/api";
import { TestCard } from "./TestCard";

const steppers = [
  { name: "stepper_x", unusable: null },
  { name: "stepper_y", unusable: "trace interface not open" },
] as StepperView[];

function card(over: Partial<Parameters<typeof TestCard>[0]> = {}) {
  return (
    <TestCard
      steppers={steppers}
      stepperName="stepper_x"
      type="probe"
      disabled={false}
      onStepper={() => {}}
      onType={() => {}}
      {...over}
    >
      <p>type settings</p>
    </TestCard>
  );
}

test("the stepper and the test type are dropdowns, one per line, above the type's settings", () => {
  render(card());
  const region = screen.getByRole("region", { name: "Test" });
  expect(within(region).getByRole("combobox", { name: "Stepper" })).toHaveTextContent("stepper_x");
  expect(within(region).getByRole("combobox", { name: "Test type" })).toHaveTextContent("Probe");
  expect(region).toHaveTextContent("type settings");
});

test("choosing another stepper or type reports it", () => {
  const onStepper = vi.fn();
  const onType = vi.fn();
  render(card({ onStepper, onType }));
  fireEvent.click(screen.getByRole("combobox", { name: "Stepper" }));
  fireEvent.click(screen.getByRole("option", { name: /stepper_y/ }));
  expect(onStepper).toHaveBeenCalledWith("stepper_y");
  fireEvent.click(screen.getByRole("combobox", { name: "Test type" }));
  fireEvent.click(screen.getByRole("option", { name: /Tracking/ }));
  expect(onType).toHaveBeenCalledWith("tracking");
});

test("unusable steppers are marked and each test type is described", () => {
  render(card());
  fireEvent.click(screen.getByRole("combobox", { name: "Stepper" }));
  expect(screen.getByRole("option", { name: /stepper_y.*unusable/ })).toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole("listbox"), { key: "Escape" });
  fireEvent.click(screen.getByRole("combobox", { name: "Test type" }));
  expect(screen.getByRole("option", { name: /Standstill/ })).toHaveTextContent(/torque/i);
});
