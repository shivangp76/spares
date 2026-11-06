import { useState } from "react";
import type { useSavedSearches } from "../hooks/useSavedSearches";
import type { SavedSearch } from "../types/spares";
import { UndoIcon } from "./Icons";
import RecentSearches from "./RecentSearches";

/**
 * The row of saved searches of the Notes, Cards and Graph pages, to run one again or delete it.
 * Deleting isn't asked about first: Undo saves it again.
 */
export default function SavedSearches({
  searches: { saved, save, remove, error },
  onSelect,
}: {
  /** `useSavedSearches()`, which the page also saves new searches with */
  searches: ReturnType<typeof useSavedSearches>;
  onSelect: (query: string) => void;
}) {
  const [deleted, setDeleted] = useState<SavedSearch | null>(null);

  return (
    <>
      <RecentSearches
        heading="Saved"
        recent={saved}
        label={(s) => s.name}
        itemTitle={(s) => s.query}
        onSelect={(s) => onSelect(s.query)}
        onRemove={(s) => {
          setDeleted(s);
          void remove(s);
        }}
      />
      {deleted && (
        <div className="status-line">
          <span>Deleted the saved search "{deleted.name}".</span>
          <button
            className="button-small"
            onClick={() => {
              void save(deleted.name, deleted.query);
              setDeleted(null);
            }}
          >
            <UndoIcon />
            Undo
          </button>
          <button className="button-small" onClick={() => setDeleted(null)}>
            Dismiss
          </button>
        </div>
      )}
      {error && <div className="error-detail">{error}</div>}
    </>
  );
}
