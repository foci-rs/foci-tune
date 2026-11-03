import { createRoute } from "@tanstack/react-router";
import { FirmwareRestart } from "@/components/printer-cards";
import { StepperCard } from "@/components/StepperCard";
import { McuCard } from "@/components/McuCard";
import { PreconditionList } from "@/components/preconditions";
import { Card } from "@/components/ui";
import { useFeed } from "@/feed";
import { Route as rootRoute } from "./__root";

export const Route = createRoute({
  getParentRoute: () => rootRoute,
  path: "/printer",
  component: PrinterPage,
});

function PrinterPage() {
  const { printer, loadError } = useFeed();

  if (printer === null) {
    return (
      <main className="mx-auto max-w-6xl px-4 py-6">
        <h1 className="text-lg font-medium text-muted-foreground">{loadError ?? "Waiting for the printer snapshot"}</h1>
      </main>
    );
  }

  const shutDown = printer.moonraker.klippy === "shutdown" || printer.moonraker.klippy === "error";

  return (
    <main className="mx-auto max-w-6xl space-y-4 px-4 py-6">
      <h1 className="sr-only">Printer</h1>
      {printer.settings_error !== null && (
        <p role="alert" className="rounded-md border border-amber-500 px-3 py-2 text-sm text-amber-300">
          {printer.settings_error}
        </p>
      )}
      {shutDown && <FirmwareRestart />}
      <div
        className="grid gap-4"
        style={{ gridTemplateColumns: `repeat(${Math.max(printer.boards.length, 1)}, minmax(0, 1fr))` }}
      >
        {printer.boards.map((board) => (
          <McuCard key={board.serial} printer={printer} board={board} />
        ))}
      </div>
      <Card label="Preconditions">
        <PreconditionList items={printer.preconditions} />
      </Card>
      {printer.steppers.length === 0 ? (
        <p className="text-sm text-muted-foreground">No FOCI steppers found.</p>
      ) : (
        <div className="grid gap-4 lg:grid-cols-2">
          {printer.steppers.map((stepper) => (
            <StepperCard
              key={stepper.name}
              stepper={stepper}
              channel={printer.boards
                .find((b) => b.serial === stepper.serial)
                ?.channels.find((c) => c.channel === stepper.channel)}
            />
          ))}
        </div>
      )}
    </main>
  );
}
