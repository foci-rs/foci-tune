import { useState } from "react";
import { ApiRequestError, postFirmwareRestart } from "@/api";
import { Button, Card } from "./ui";

export function FirmwareRestart() {
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const restart = () => {
    setBusy(true);
    setMessage(null);
    postFirmwareRestart().then(
      () => {
        setBusy(false);
        setMessage("Firmware restart requested.");
      },
      (e: unknown) => {
        setBusy(false);
        setMessage(e instanceof ApiRequestError ? e.message : "request failed");
      },
    );
  };
  return (
    <Card label="Klipper is shut down" className="border-red-500">
      <p className="mb-3 text-sm">After a restart the gains are whatever the config says.</p>
      <Button tone="danger" disabled={busy} onClick={restart}>
        Firmware restart
      </Button>
      {message !== null && (
        <p role="status" className="mt-2 text-sm text-muted-foreground">
          {message}
        </p>
      )}
    </Card>
  );
}
