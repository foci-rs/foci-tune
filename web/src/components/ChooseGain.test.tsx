import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ChooseGain } from "./ChooseGain";

const gains = [
  { index: 0, label: "421" },
  { index: 1, label: "530" },
  { index: 2, label: "668" },
];

function choose(over: Partial<Parameters<typeof ChooseGain>[0]> = {}) {
  const handlers = { onAccept: vi.fn(), onPick: vi.fn(), onManual: vi.fn() };
  render(
    <TooltipProvider>
      <ChooseGain
        axis="VelocityP"
        accept="530"
        initial={530}
        gains={gains}
        pwmHz={50000}
        busy={false}
        {...handlers}
        {...over}
      />
    </TooltipProvider>,
  );
  return handlers;
}

const openMenu = () => fireEvent.keyDown(screen.getByRole("button", { name: "More ways to choose" }), { key: "Enter" });

test("the main button just says Accept and takes the recommended gain", () => {
  const { onAccept } = choose();
  fireEvent.click(screen.getByRole("button", { name: "Accept" }));
  expect(onAccept).toHaveBeenCalledTimes(1);
  expect(screen.queryByText("530")).toBeNull();
});

test("hovering Accept says which gain it takes, without printing it on the button", async () => {
  choose();
  fireEvent.focus(screen.getByRole("button", { name: "Accept" }).parentElement as HTMLElement);
  expect(await screen.findByRole("tooltip")).toHaveTextContent("Take the recommended gain, 530.");
});

test("the menu lists the measured rungs with their gains, none of them tagged, then a way to type one", () => {
  choose();
  openMenu();
  expect(screen.getAllByRole("menuitem").map((i) => i.textContent)).toEqual([
    "Rung 1: 421",
    "Rung 2: 530",
    "Rung 3: 668",
    "Enter a value…",
  ]);
});

test("picking a gain from the menu chooses that gain", () => {
  const { onPick } = choose();
  openMenu();
  fireEvent.click(screen.getByRole("menuitem", { name: "Rung 3: 668" }));
  expect(onPick).toHaveBeenCalledWith(2);
});

test("Enter a value opens a dialog with the recommended gain filled in, and Confirm sends the typed value", () => {
  const { onManual } = choose();
  openMenu();
  fireEvent.click(screen.getByRole("menuitem", { name: "Enter a value…" }));
  const dialog = screen.getByRole("dialog");
  const input = within(dialog).getByRole("spinbutton", { name: "Gain" });
  expect(input).toHaveValue(530);
  fireEvent.change(input, { target: { value: "612" } });
  fireEvent.click(within(dialog).getByRole("button", { name: "Confirm" }));
  expect(onManual).toHaveBeenCalledWith(612);
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("the dialog holds Confirm back with the reason while the value cannot be used, and Cancel closes it", () => {
  const { onManual } = choose();
  openMenu();
  fireEvent.click(screen.getByRole("menuitem", { name: "Enter a value…" }));
  const dialog = screen.getByRole("dialog");
  fireEvent.change(within(dialog).getByRole("spinbutton", { name: "Gain" }), { target: { value: "0" } });
  expect(within(dialog).getByRole("button", { name: "Confirm" })).toBeDisabled();
  expect(within(dialog).getByText("The gain must be above zero.")).toBeInTheDocument();
  fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(onManual).not.toHaveBeenCalled();
});

test("with no recommendation the main button has nothing to accept, but gains can still be picked", () => {
  choose({ accept: null, initial: null, gains: [gains[0]] });
  expect(screen.getByRole("button", { name: "Accept" })).toBeDisabled();
  openMenu();
  expect(screen.getByRole("menuitem", { name: "Rung 1: 421" })).toBeInTheDocument();
});

test("while busy nothing can be pressed", () => {
  choose({ busy: true });
  expect(screen.getByRole("button", { name: "Accept" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "More ways to choose" })).toBeDisabled();
});
