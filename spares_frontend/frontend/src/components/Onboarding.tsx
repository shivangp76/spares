import { Link } from "react-router-dom";
import { docUrl } from "../help/pages";
import { STARTERS, type StarterParser } from "../onboarding";

/**
 * The first steps of a new, empty collection, on the review page: write a note in one of the
 * markups (or add sample notes, tagged `sample`), review it, and set up sync.
 */
export default function Onboarding({
  busy,
  onWrite,
  onSamples,
}: {
  busy: boolean;
  /** Opens a first note in `parser`, to review once created */
  onWrite: (parser: StarterParser) => void;
  /** Adds a sample note in each markup, and reviews them */
  onSamples: () => void;
}) {
  return (
    <section className="onboarding" aria-labelledby="onboarding-title">
      <h2 id="onboarding-title" className="onboarding-title">
        Getting started
      </h2>
      <ol className="onboarding-steps">
        <li>
          <strong>Write a note.</strong> Each part you hide with a cloze becomes
          a card. Choose its markup:
          <span className="onboarding-actions">
            {STARTERS.map(({ parser, label }) => (
              <button
                key={parser}
                onClick={() => onWrite(parser)}
                disabled={busy}
                className="touch-target-small"
              >
                {label}
              </button>
            ))}
            <span className="onboarding-or">or</span>
            <button
              onClick={onSamples}
              disabled={busy}
              className="touch-target-small"
              title="One note in each markup, tagged sample, so they are easy to delete"
            >
              Add sample notes
            </button>
          </span>
        </li>
        <li>
          <strong>Review it.</strong> Creating it starts a review: show the
          answer, then rate how well you recalled it. Spares brings each card
          back just before you would forget it.
        </li>
        <li>
          <strong>Set up sync</strong>, to review on your phone and keep the
          collection on a server of your own: see{" "}
          <Link
            to={docUrl({
              label: "Deploying a sync server",
              file: "deploying.md",
            })}
          >
            Deploying a sync server
          </Link>
          .
        </li>
      </ol>
    </section>
  );
}
