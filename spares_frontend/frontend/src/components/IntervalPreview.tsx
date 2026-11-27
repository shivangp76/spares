import { useEffect, useState } from "react";
import { previewIntervals } from "../api/client";
import { formatSteps, parseSteps } from "../intervalPreview";
import type { IntervalPreviewResponse } from "../types/spares";
import { errorMessage, formatInterval } from "../utils";

const DAY = 86_400;

/** The settings as typed, before they are read */
interface Draft {
  retention: string;
  learning: string;
  relearning: string;
  maximumDays: string;
}

/**
 * "Try a card": the next interval of each rating for a new card and one seen a few times, with
 * config.toml's scheduler settings, or with other values typed here to see what they would do.
 * Nothing typed here is saved: the cards are scheduled with config.toml's.
 */
export default function IntervalPreview({
  schedulerName,
}: {
  schedulerName: string;
}) {
  const [draft, setDraft] = useState<Draft | null>(null);
  const [preview, setPreview] = useState<IntervalPreviewResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  // What to ask for: config.toml's settings until one is changed
  const request = draft && {
    desired_retention: Number(draft.retention),
    learning_steps: parseSteps(draft.learning),
    relearning_steps: parseSteps(draft.relearning),
    maximum_interval: Math.round(Number(draft.maximumDays) * DAY),
  };
  const invalid =
    request &&
    (!Number.isFinite(request.desired_retention) ||
      request.learning_steps === null ||
      request.relearning_steps === null ||
      !Number.isFinite(request.maximum_interval));
  const key = invalid ? null : JSON.stringify(request ?? {});

  useEffect(() => {
    if (key === null) return;
    let cancelled = false;
    // A moment after the last change, so typing a number doesn't ask for each digit
    const timer = setTimeout(
      () => {
        previewIntervals(schedulerName, JSON.parse(key)).then(
          (response) => {
            if (cancelled) return;
            setPreview(response);
            setError(null);
            // The fields start from what config.toml has
            setDraft(
              (d) =>
                d ?? {
                  retention: String(response.desired_retention),
                  learning: formatSteps(response.learning_steps),
                  relearning: formatSteps(response.relearning_steps),
                  maximumDays: String(response.maximum_interval / DAY),
                },
            );
          },
          (e) => {
            if (!cancelled) setError(errorMessage(e));
          },
        );
      },
      draft ? 300 : 0,
    );
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
    // `draft` only says whether to wait, which `key` changing already covers
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, schedulerName]);

  const set =
    (field: keyof Draft) => (e: React.ChangeEvent<HTMLInputElement>) =>
      setDraft((d) => d && { ...d, [field]: e.target.value });

  return (
    <div className="interval-preview">
      <div className="settings-choices" style={{ marginBottom: 6 }}>
        <span>Try a card:</span>
      </div>
      {draft && (
        <div className="interval-preview-fields">
          <label>
            Desired retention{" "}
            <input
              type="number"
              min={0.7}
              max={0.99}
              step={0.01}
              value={draft.retention}
              onChange={set("retention")}
              style={{ width: 64 }}
            />
          </label>
          <label>
            Learning steps{" "}
            <input
              value={draft.learning}
              onChange={set("learning")}
              placeholder="1m 10m"
              size={8}
              aria-invalid={parseSteps(draft.learning) === null}
            />
          </label>
          <label>
            Relearning steps{" "}
            <input
              value={draft.relearning}
              onChange={set("relearning")}
              placeholder="10m"
              size={8}
              aria-invalid={parseSteps(draft.relearning) === null}
            />
          </label>
          <label>
            Maximum interval{" "}
            <input
              type="number"
              min={1}
              step={1}
              value={draft.maximumDays}
              onChange={set("maximumDays")}
              style={{ width: 64 }}
            />{" "}
            days
          </label>
        </div>
      )}
      {preview && (
        <table className="interval-preview-table">
          <tbody>
            {preview.examples.map((example) => (
              <tr key={example.label}>
                <th scope="row">{example.label}</th>
                {example.intervals.map(({ rating, seconds }) => (
                  <td key={rating.id}>
                    <span className="interval-preview-rating">
                      {rating.description}
                    </span>{" "}
                    {formatInterval(seconds)}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {(error || invalid) && (
        <p className="error-text" style={{ fontSize: 13, margin: "6px 0 0" }}>
          {invalid
            ? "Steps are durations like 1m 10m, in seconds (s), minutes (m) or hours (h)."
            : error}
        </p>
      )}
      <p className="field-hint">
        The next interval after each rating, for a new card and one rated Good
        each time it was due. It starts from config.toml&apos;s settings: try
        others here to see what they would do. Nothing here is saved. Load
        balancing may move a card by a day or so.
      </p>
    </div>
  );
}
