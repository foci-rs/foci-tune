import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { BedMap } from "./BedMap";

const limits = { x: [0, 300], y: [0, 300] } as never;

test("the proposal line carries the title the page gives it", () => {
  render(
    <BedMap
      limits={limits}
      pattern={{ start: [50, 60], end: [250, 70] }}
      proposal={{ start: [60, 60], end: [240, 60] }}
      proposalTitle="Default path"
      onChange={() => {}}
    />,
  );
  expect(screen.getByText("Default path")).toBeInTheDocument();
  expect(screen.queryByText("Proposed pattern")).toBeNull();
});

test("the legend paragraph is gone; the margin box explains itself on hover", () => {
  render(<BedMap limits={limits} pattern={{ start: [50, 60], end: [250, 70] }} proposal={null} onChange={() => {}} />);
  expect(screen.queryByText(/Start is green/)).toBeNull();
  expect(screen.getByText("10 mm margin")).toBeInTheDocument();
});

test("the pattern length is shown and follows the coordinates", () => {
  const { rerender } = render(
    <BedMap limits={limits} pattern={{ start: [50, 60], end: [250, 70] }} proposal={null} onChange={() => {}} />,
  );
  const legend = screen.getByText(/^Length/);
  expect(legend).toHaveTextContent("Length 200 mm");
  expect(legend).toHaveClass("text-center");
  const map = screen.getByRole("img");
  expect(map.compareDocumentPosition(legend) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(
    legend.compareDocumentPosition(screen.getByRole("group", { name: "Start" })) & Node.DOCUMENT_POSITION_FOLLOWING,
  ).toBeTruthy();
  rerender(
    <BedMap limits={limits} pattern={{ start: [50, 50], end: [250, 250] }} proposal={null} onChange={() => {}} />,
  );
  expect(screen.getByText(/^Length/)).toHaveTextContent("Length 283 mm");
});

test("coordinates are whole millimetres: shown rounded, stepped by 1 and typed values rounded", () => {
  const onChange = vi.fn();
  render(
    <BedMap limits={limits} pattern={{ start: [50.4, 60], end: [250, 70] }} proposal={null} onChange={onChange} />,
  );
  const x = screen.getByRole("spinbutton", { name: "Start X" });
  expect(x).toHaveValue(50);
  expect(x).toHaveAttribute("step", "1");
  fireEvent.change(screen.getByRole("spinbutton", { name: "End Y" }), { target: { value: "80.6" } });
  expect(onChange).toHaveBeenCalledWith({ start: [50.4, 60], end: [250, 81] });
});

test("start and end each get their own row with an X and a Y field", () => {
  const onChange = vi.fn();
  render(<BedMap limits={limits} pattern={{ start: [50, 60], end: [250, 70] }} proposal={null} onChange={onChange} />);
  const start = screen.getByRole("group", { name: "Start" });
  const end = screen.getByRole("group", { name: "End" });
  expect(
    within(start)
      .getAllByRole("spinbutton")
      .map((i) => i.getAttribute("aria-label")),
  ).toEqual(["Start X", "Start Y"]);
  expect(
    within(end)
      .getAllByRole("spinbutton")
      .map((i) => i.getAttribute("aria-label")),
  ).toEqual(["End X", "End Y"]);
  fireEvent.change(within(end).getByRole("spinbutton", { name: "End Y" }), { target: { value: "80" } });
  expect(onChange).toHaveBeenCalledWith({ start: [50, 60], end: [250, 80] });
});
