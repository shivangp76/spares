import { REPOSITORY_URL } from "../demo";
import { resetDemo } from "../local/demo";

/** Above the navbar in the demo (src/demo.ts): what it is, starting it over, and where spares is. */
export default function DemoBanner() {
  return (
    <div className="demo-banner" role="note">
      <span>
        <strong>Demo</strong>: example notes with made-up reviews, kept in this
        browser only. Review, edit, search and look around.
      </span>
      <span className="demo-banner-actions">
        <button
          className="button-small"
          onClick={() => void resetDemo()}
          title="Delete your changes to the demo and load it again"
        >
          Reset
        </button>
        <a href={REPOSITORY_URL}>Get spares</a>
      </span>
    </div>
  );
}
