import { CheckCircle2Icon, CircleIcon } from "lucide-react";
import { Card } from "./ui";

export function GainChecklist({ items }: { items: { label: string; done: boolean }[] }) {
  return (
    <ul aria-label="Gains chosen" className="flex flex-row flex-wrap items-center text-sm">
      {items.map((item) => (
        <li
          key={item.label}
          data-done={item.done}
          className="flex items-center gap-1.5 after:mx-3 after:text-muted-foreground after:content-['·'] last:after:content-none"
        >
          {item.done ? (
            <CheckCircle2Icon aria-hidden="true" className="size-4 text-emerald-400" />
          ) : (
            <CircleIcon aria-hidden="true" className="size-4 text-muted-foreground" />
          )}
          <span className={item.done ? "" : "text-muted-foreground"}>{item.label}</span>
          <span className="sr-only">: {item.done ? "chosen" : "not chosen yet"}</span>
        </li>
      ))}
    </ul>
  );
}

export function ResultPlaceholder({ items }: { items: { label: string; done: boolean }[] }) {
  return (
    <Card label="Result">
      <GainChecklist items={items} />
    </Card>
  );
}
