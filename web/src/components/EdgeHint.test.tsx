import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import type { ProbeEdgeHint } from "@/tune";
import { EdgeHintLine, ProbeNotes } from "./EdgeHint";

const show = (hint: ProbeEdgeHint) => render(<EdgeHintLine hint={hint} />);

test("a healthy estimate shows nothing", () => {
  const { container } = show({
    tone: "ok",
    edges: 137,
    text: "This setup should record about 137 step responses.",
  });
  expect(container).toBeEmptyDOMElement();
});

test("a low estimate shows the full advice", () => {
  show({
    tone: "warning",
    edges: 12,
    text: "This setup should record only about 12 step responses, and the test needs 20. Use 25 strokes.",
  });
  expect(screen.getByRole("note")).toHaveTextContent("Use 25 strokes");
});

test("a borderline estimate is a caution", () => {
  show({ tone: "caution", edges: 33, text: "That is enough to run, but 40 or more is steadier." });
  expect(screen.getByRole("note")).toHaveTextContent("enough to run");
});

test("a spacing problem shows as a caution and takes the place of the estimate", () => {
  render(
    <ProbeNotes
      spacingProblem="At 400 mm/s gain changes are 75 ms apart."
      hint={{ tone: "warning", edges: 0, text: "This setup should record only about 0 step responses." }}
    />,
  );
  expect(screen.getAllByRole("note")).toHaveLength(1);
  expect(screen.getByRole("note")).toHaveTextContent("75 ms apart");
});

test("without a spacing problem the estimate shows", () => {
  render(<ProbeNotes spacingProblem={null} hint={{ tone: "caution", edges: 33, text: "enough to run" }} />);
  expect(screen.getByRole("note")).toHaveTextContent("enough to run");
});
