import type { ReactNode } from "react";
import type { StepperView } from "@/api";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import type { RunType } from "@/tune";
import { Card } from "./ui";

const RUN_TYPES: { type: RunType; label: string; description: string }[] = [
  { type: "tracking", label: "Tracking", description: "Following error along the pattern at set speeds" },
  { type: "probe", label: "Probe", description: "Step response of the velocity loop and its damping" },
  { type: "standstill", label: "Standstill", description: "Torque noise while holding position" },
];

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="space-y-1">
      <p className="text-[0.65rem] uppercase tracking-wider text-muted-foreground">{label}</p>
      {children}
    </div>
  );
}

/** Which stepper to test and with which kind of test, above that test's own settings. */
export function TestCard({
  steppers,
  stepperName,
  type,
  disabled,
  onStepper,
  onType,
  children,
}: {
  steppers: StepperView[];
  stepperName: string;
  type: RunType;
  disabled: boolean;
  onStepper: (name: string) => void;
  onType: (type: RunType) => void;
  children?: ReactNode;
}) {
  const chosen = RUN_TYPES.find((t) => t.type === type);
  return (
    <Card label="Test setup">
      <div className="space-y-3">
        <Row label="Stepper">
          <Select value={stepperName} disabled={disabled} onValueChange={onStepper}>
            <SelectTrigger aria-label="Stepper" className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {steppers.map((s) => (
                <SelectItem key={s.name} value={s.name}>
                  {s.name}
                  {s.unusable !== null ? " (unusable)" : ""}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Row>
        <Row label="Test">
          <Select value={type} disabled={disabled} onValueChange={(v) => onType(v as RunType)}>
            <SelectTrigger aria-label="Test" className="w-full">
              <SelectValue>{chosen?.label}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              {RUN_TYPES.map((t) => (
                <SelectItem key={t.type} value={t.type}>
                  <span className="flex flex-col items-start">
                    {t.label}
                    <span className="text-xs text-muted-foreground">{t.description}</span>
                  </span>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {chosen !== undefined && <p className="text-xs text-muted-foreground">{chosen.description}</p>}
        </Row>
        {children}
      </div>
    </Card>
  );
}
