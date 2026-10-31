import type { PreconditionView } from "@/api";

export function PreconditionList({ items }: { items: PreconditionView[] }) {
  if (items.length === 0) {
    return <p className="text-sm text-muted-foreground">No steppers to check.</p>;
  }
  return (
    <div className="space-y-3">
      {items.map((item) => (
        <div
          key={item.stepper}
          role="group"
          aria-label={item.stepper}
          className="rounded-md border p-3"
        >
          <p className="font-mono text-sm">{item.stepper}</p>
          {item.ok ? (
            <p className="text-sm text-emerald-400">All preconditions hold</p>
          ) : (
            <ul className="mt-1 list-disc pl-5 text-sm text-amber-400">
              {item.failing.map((reason) => (
                <li key={reason}>{reason}</li>
              ))}
            </ul>
          )}
        </div>
      ))}
    </div>
  );
}
