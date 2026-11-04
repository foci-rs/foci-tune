import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { Steps } from "./Steps";

const two = [
  { id: "prepare", label: "Prepare" },
  { id: "run", label: "Run" },
];
const four = [
  { id: "setup", label: "Setup" },
  { id: "sweep", label: "Sweep" },
  { id: "review", label: "Review" },
  { id: "apply", label: "Apply" },
];

test("it lists as many steps as it is given, numbered in order", () => {
  const { rerender } = render(<Steps steps={two} current="prepare" onSelect={() => {}} />);
  let items = within(screen.getByRole("list")).getAllByRole("listitem");
  expect(items.map((i) => i.textContent)).toEqual(["1Prepare", "2Run"]);
  rerender(<Steps steps={four} current="setup" onSelect={() => {}} />);
  items = within(screen.getByRole("list")).getAllByRole("listitem");
  expect(items.map((i) => i.textContent)).toEqual(["1Setup", "2Sweep", "3Review", "4Apply"]);
});

test("the current step is marked, earlier ones are done and later ones are upcoming", () => {
  render(<Steps steps={four} current="review" onSelect={() => {}} />);
  const state = (name: string) => screen.getByRole("button", { name: new RegExp(name) }).getAttribute("data-state");
  expect(state("Setup")).toBe("done");
  expect(state("Sweep")).toBe("done");
  expect(state("Review")).toBe("current");
  expect(state("Apply")).toBe("upcoming");
  expect(screen.getByRole("button", { name: /Review/ })).toHaveAttribute("aria-current", "step");
  expect(screen.getByRole("button", { name: /Apply/ })).not.toHaveAttribute("aria-current");
});

test("selecting a step reports its id", () => {
  const onSelect = vi.fn();
  render(<Steps steps={two} current="prepare" onSelect={onSelect} />);
  fireEvent.click(screen.getByRole("button", { name: /Run/ }));
  expect(onSelect).toHaveBeenCalledWith("run");
  fireEvent.click(screen.getByRole("button", { name: /Prepare/ }));
  expect(onSelect).toHaveBeenCalledWith("prepare");
});

test("a current step that is not in the list marks none", () => {
  render(<Steps steps={two} current="elsewhere" onSelect={() => {}} />);
  for (const b of screen.getAllByRole("button")) expect(b).not.toHaveAttribute("aria-current");
});

test("every circle is the same size and an upcoming one is filled like the card", () => {
  render(<Steps steps={four} current="sweep" onSelect={() => {}} />);
  const circle = (name: RegExp) => screen.getByRole("button", { name }).firstElementChild as HTMLElement;
  for (const name of [/Setup/, /Sweep/, /Review/, /Apply/]) {
    expect(circle(name)).toHaveClass("size-7", "border-2");
  }
  expect(circle(/Review/)).toHaveClass("bg-card");
  expect(circle(/Apply/)).toHaveClass("bg-card");
  expect(circle(/Sweep/)).not.toHaveClass("bg-card");
});

test("every step gets an equal share of the row, whatever its label", () => {
  const { rerender } = render(<Steps steps={two} current="prepare" onSelect={() => {}} />);
  expect(screen.getByRole("list").style.gridTemplateColumns).toBe("repeat(2, minmax(0, 1fr))");
  rerender(<Steps steps={four} current="setup" onSelect={() => {}} />);
  expect(screen.getByRole("list").style.gridTemplateColumns).toBe("repeat(4, minmax(0, 1fr))");
});
