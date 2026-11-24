import { useEffect, useState } from "react";
import { chooseScheduler, getCredentials, listSchedulers } from "../api/client";
import { errorMessage } from "../utils";

/** The scheduler this device rates cards with, of the host's, kept across logging in again. */
export default function SchedulerSettings() {
  const [current, setCurrent] = useState(() => {
    try {
      return getCredentials().schedulerName;
    } catch {
      return null;
    }
  });
  // The host's, its default first
  const [schedulers, setSchedulers] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    listSchedulers().then(
      (names) => {
        if (!cancelled) setSchedulers(names);
      },
      (e: unknown) => {
        if (!cancelled) setError(errorMessage(e));
      },
    );
    return () => {
      cancelled = true;
    };
  }, []);

  if (current === null) return null;
  if (schedulers?.length === 1 && schedulers[0] === current)
    return (
      <>
        <div className="settings-choices">
          <span>Scheduler:</span>
          <span>{current}</span>
        </div>
        <p className="field-hint">
          What schedules the cards you rate. The host has no other.
        </p>
      </>
    );
  return (
    <>
      <div className="settings-choices">
        <span>Scheduler:</span>
        {(schedulers ?? [current]).map((name, i) => (
          <label key={name} className="checkbox-label">
            <input
              type="radio"
              name="scheduler"
              value={name}
              checked={current === name}
              // Until the host lists its own, only the one in use is shown
              disabled={schedulers === null}
              onChange={() => {
                if (!schedulers) return;
                chooseScheduler(name, schedulers);
                setCurrent(name);
              }}
            />
            {name}
            {schedulers && i === 0 && " (default)"}
          </label>
        ))}
      </div>
      <p className="field-hint">
        {error
          ? `The host's schedulers could not be listed: ${error}`
          : "What schedules the cards you rate on this device, and whose statistics are shown. Logging in again keeps it, if the host has it."}
      </p>
    </>
  );
}
