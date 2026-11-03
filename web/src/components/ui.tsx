import { useEffect, useRef, useState, type ButtonHTMLAttributes, type ReactNode } from "react";
import { cn } from "@/lib/utils";

export function Card({
  label,
  action,
  children,
  className,
}: {
  label: string;
  action?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section aria-label={label} className={cn("rounded-xl border bg-card p-4 text-card-foreground", className)}>
      <div className="mb-3 flex items-center justify-between gap-2">
        <h2 className="text-xs font-medium uppercase tracking-wider text-muted-foreground">{label}</h2>
        {action}
      </div>
      {children}
    </section>
  );
}

export function Field({ label, children, sub }: { label: string; children: ReactNode; sub?: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-[0.65rem] uppercase tracking-wider text-muted-foreground">{label}</dt>
      <dd className="font-mono text-sm">
        {children}
        {sub !== undefined && <span className="ml-2 text-muted-foreground">{sub}</span>}
      </dd>
    </div>
  );
}

export function Dot({ tone }: { tone: "ok" | "warn" | "error" | "off" }) {
  const color =
    tone === "ok"
      ? "bg-emerald-400"
      : tone === "warn"
        ? "bg-amber-400"
        : tone === "error"
          ? "bg-red-400"
          : "bg-muted-foreground/50";
  return <span aria-hidden="true" className={cn("inline-block size-2 rounded-full", color)} />;
}

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  tone?: "neutral" | "danger";
};

export function Button({ tone = "neutral", className, ...props }: ButtonProps) {
  return (
    <button
      type="button"
      className={cn(
        "rounded-md border px-3 py-1.5 text-sm font-medium transition-colors focus-visible:outline-2 disabled:cursor-not-allowed disabled:opacity-50",
        tone === "danger"
          ? "border-red-500 bg-red-600 text-white hover:bg-red-500"
          : "bg-secondary text-secondary-foreground hover:bg-accent",
        className,
      )}
      {...props}
    />
  );
}

export function EmptyState({ heading }: { heading: string }) {
  return (
    <main className="mx-auto max-w-5xl px-4 py-16 text-center">
      <h1 className="text-lg font-medium text-muted-foreground">{heading}</h1>
    </main>
  );
}

/**
 * A numeric input that reports every parseable value and keeps the operator's text while
 * focused, so a value derived from the input does not rewrite it mid-typing.
 */
export function NumberField({
  label,
  value,
  onValue,
  step,
  disabled = false,
  className,
}: {
  label: string;
  value: number;
  onValue: (v: number) => void;
  step?: number;
  disabled?: boolean;
  className?: string;
}) {
  const [text, setText] = useState(String(value));
  const focused = useRef(false);
  useEffect(() => {
    if (!focused.current) setText(String(value));
  }, [value]);
  return (
    <input
      type="number"
      aria-label={label}
      step={step}
      value={text}
      disabled={disabled}
      onFocus={() => (focused.current = true)}
      onBlur={() => {
        focused.current = false;
        setText(String(value));
      }}
      onChange={(e) => {
        setText(e.target.value);
        const v = Number(e.target.value);
        if (e.target.value.trim() !== "" && Number.isFinite(v)) onValue(v);
      }}
      className={cn(
        "w-24 rounded border bg-background px-1.5 py-0.5 font-mono text-sm text-foreground disabled:opacity-50",
        className,
      )}
    />
  );
}
