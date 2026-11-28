import { type ReactNode, useMemo, useState } from "react";
import {
  displayValue,
  type Draft,
  FIELD_GROUPS,
  type FieldKind,
  GOAL_FIELDS,
  parseInput,
  pathKey,
  PRESET_FIELDS,
  type SettingPath,
  valueAt,
} from "../collectionSettings";
import type { SettingsView } from "../types/spares";
import { errorMessage } from "../utils";

/** An edited input: its text, and what it sets (or why it can't). */
interface Edit {
  path: SettingPath;
  text: string;
  draft: Draft | null;
  error: string | null;
}

/**
 * The common settings of the collection as a form, saved together as one change. A field shows
 * the value in effect, defaults included; one edited back to what it was is no change.
 */
export default function SettingsForm({
  view,
  onSave,
}: {
  view: SettingsView;
  onSave: (drafts: Draft[]) => Promise<void>;
}) {
  const [edits, setEdits] = useState<Map<string, Edit>>(new Map());
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [newPresets, setNewPresets] = useState<string[]>([]);
  const [newGoals, setNewGoals] = useState<string[]>([]);

  const drafts = useMemo(
    () =>
      [...edits.values()].flatMap((edit) => (edit.draft ? [edit.draft] : [])),
    [edits],
  );
  const invalid = [...edits.values()].some((edit) => edit.error !== null);
  const removed = (path: SettingPath) =>
    edits.get(pathKey(path))?.draft?.remove === true;

  function edit(path: SettingPath, kind: FieldKind, text: string) {
    const original = displayValue(kind, valueAt(view.values, path));
    setEdits((edits) => {
      const next = new Map(edits);
      if (text === original) {
        next.delete(pathKey(path));
        return next;
      }
      const parsed = parseInput(kind, text);
      next.set(pathKey(path), {
        path,
        text,
        error: "error" in parsed ? parsed.error : null,
        draft:
          "error" in parsed
            ? null
            : "remove" in parsed
              ? { path, value: undefined, remove: true }
              : { path, value: parsed.value, remove: false },
      });
      return next;
    });
  }

  function setRemoved(path: SettingPath, remove: boolean) {
    setEdits((edits) => {
      const next = new Map(edits);
      if (remove)
        next.set(pathKey(path), {
          path,
          text: "",
          error: null,
          draft: { path, value: undefined, remove: true },
        });
      else next.delete(pathKey(path));
      return next;
    });
  }

  async function save() {
    setSaving(true);
    setSaveError(null);
    try {
      await onSave(drafts);
    } catch (e) {
      setSaveError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  }

  const field = (
    path: SettingPath,
    label: string,
    kind: FieldKind,
    hint?: string,
  ) => {
    const current = edits.get(pathKey(path));
    return (
      <Field
        key={pathKey(path)}
        label={label}
        kind={kind}
        hint={hint}
        text={current?.text ?? displayValue(kind, valueAt(view.values, path))}
        error={current?.error ?? null}
        changed={current !== undefined}
        onChange={(text) => edit(path, kind, text)}
      />
    );
  };

  const presets = Object.keys(
    (valueAt(view.values, ["queue", "presets"]) as
      Record<string, unknown> | undefined) ?? {},
  );
  const goals = Object.keys(
    (valueAt(view.values, ["goals"]) as Record<string, unknown>) ?? {},
  );

  return (
    <div className="settings-form">
      {FIELD_GROUPS.map((group) => (
        <section key={group.title} className="settings-group">
          <h3 className="settings-subheading">{group.title}</h3>
          {group.fields.map((spec) =>
            field(spec.path, spec.label, spec.kind, spec.hint),
          )}
          {group.title === "Steps and intervals" && (
            <FsrsParameters view={view} />
          )}
        </section>
      ))}

      <section className="settings-group">
        <h3 className="settings-subheading">Presets</h3>
        <p className="field-hint">
          Settings of their own for the cards of notes with some tags, or of a
          search. A card uses the first preset it matches; settings left empty
          follow the ones above.
        </p>
        {[
          ...presets,
          ...newPresets.filter((name) => !presets.includes(name)),
        ].map((name) => {
          const path = ["queue", "presets", name];
          return (
            <Entry
              key={name}
              name={name}
              isNew={!presets.includes(name)}
              removed={removed(path)}
              onRemove={(remove) => {
                if (!presets.includes(name))
                  setNewPresets((names) => names.filter((n) => n !== name));
                setRemoved(path, remove);
              }}
            >
              {PRESET_FIELDS.map((spec) =>
                field([...path, spec.key], spec.label, spec.kind, spec.hint),
              )}
            </Entry>
          );
        })}
        <AddEntry
          label="Add a preset"
          taken={[...presets, ...newPresets]}
          onAdd={(name) => setNewPresets((names) => [...names, name])}
        />
      </section>

      <section className="settings-group">
        <h3 className="settings-subheading">Goals</h3>
        <p className="field-hint">
          Cards to know by a date, such as an exam&apos;s, reported on the
          Statistics page. Their milestones are in the TOML.
        </p>
        {[...goals, ...newGoals.filter((name) => !goals.includes(name))].map(
          (name) => {
            const path = ["goals", name];
            return (
              <Entry
                key={name}
                name={name}
                isNew={!goals.includes(name)}
                removed={removed(path)}
                onRemove={(remove) => {
                  if (!goals.includes(name))
                    setNewGoals((names) => names.filter((n) => n !== name));
                  setRemoved(path, remove);
                }}
              >
                {GOAL_FIELDS.map((spec) =>
                  field([...path, spec.key], spec.label, spec.kind, spec.hint),
                )}
              </Entry>
            );
          },
        )}
        <AddEntry
          label="Add a goal"
          taken={[...goals, ...newGoals]}
          onAdd={(name) => setNewGoals((names) => [...names, name])}
        />
      </section>

      {saveError && <p className="error-text-small">{saveError}</p>}
      <div className="settings-row settings-save-row">
        <button
          onClick={() => void save()}
          disabled={drafts.length === 0 || invalid || saving}
        >
          {saving
            ? "Saving…"
            : drafts.length === 0
              ? "Save"
              : `Save ${drafts.length} change${drafts.length === 1 ? "" : "s"}`}
        </button>
        <button
          onClick={() => {
            setEdits(new Map());
            setNewPresets([]);
            setNewGoals([]);
            setSaveError(null);
          }}
          disabled={
            edits.size === 0 && newPresets.length === 0 && newGoals.length === 0
          }
        >
          Discard changes
        </button>
        {invalid && (
          <span className="error-text-small">
            Correct the fields marked first.
          </span>
        )}
      </div>
    </div>
  );
}

function Field({
  label,
  kind,
  hint,
  text,
  error,
  changed,
  onChange,
}: {
  label: string;
  kind: FieldKind;
  hint?: string;
  text: string;
  error: string | null;
  changed: boolean;
  onChange: (text: string) => void;
}) {
  const className = `settings-field${changed ? " settings-field-changed" : ""}`;
  if (kind.type === "bool")
    return (
      <div className={className}>
        <label className="checkbox-label">
          <input
            type="checkbox"
            checked={text === "true"}
            onChange={(e) => onChange(JSON.stringify(e.target.checked))}
          />
          {label}
        </label>
        {hint && <p className="field-hint">{hint}</p>}
      </div>
    );
  let input;
  if (kind.type === "select")
    input = (
      <select value={text} onChange={(e) => onChange(e.target.value)}>
        {kind.optional !== undefined && (
          <option value="">{kind.optional}</option>
        )}
        {kind.options.map((option) => (
          <option
            key={JSON.stringify(option.value)}
            value={JSON.stringify(option.value)}
          >
            {option.label}
          </option>
        ))}
      </select>
    );
  else if (kind.type === "parts")
    input = (
      <textarea
        value={text}
        rows={Math.max(2, text.split("\n").length)}
        onChange={(e) => onChange(e.target.value)}
        placeholder="probability: tag=probability"
        spellCheck={false}
      />
    );
  else {
    const numeric =
      kind.type === "number" ||
      kind.type === "limit" ||
      kind.type === "duration";
    input = (
      <input
        type={kind.type === "date" ? "date" : "text"}
        inputMode={
          numeric
            ? kind.type === "number" && kind.fraction
              ? "decimal"
              : "numeric"
            : undefined
        }
        value={text}
        placeholder={
          kind.type === "text"
            ? kind.placeholder
            : kind.type === "steps"
              ? kind.optional
                ? "As above"
                : "1m 10m"
              : kind.type === "limit" || ("optional" in kind && kind.optional)
                ? "None"
                : undefined
        }
        onChange={(e) => onChange(e.target.value)}
        aria-invalid={error !== null}
        spellCheck={false}
        size={
          kind.type === "text" || kind.type === "tags" || kind.type === "steps"
            ? 24
            : 8
        }
        className={
          kind.type === "text" && kind.wide
            ? "settings-search-input"
            : undefined
        }
      />
    );
  }
  const unit = kind.type === "duration" ? kind.unit : null;
  return (
    <div className={className}>
      <label className="settings-row">
        <span className="settings-field-label">{label}</span>
        {input}
        {unit && <span className="field-unit">{unit}</span>}
      </label>
      {error ? (
        <p className="error-text-small">{error}</p>
      ) : (
        hint && <p className="field-hint">{hint}</p>
      )}
    </div>
  );
}

/** A preset or goal, which can be removed with all its settings. */
function Entry({
  name,
  isNew,
  removed,
  onRemove,
  children,
}: {
  name: string;
  /** Added in this form, so shown open */
  isNew: boolean;
  removed: boolean;
  onRemove: (remove: boolean) => void;
  children: ReactNode;
}) {
  return (
    <details className="settings-entry" open={isNew || undefined}>
      <summary>
        <span className={removed ? "settings-entry-removed" : undefined}>
          {name}
        </span>
        <button
          className="button-small"
          onClick={(e) => {
            e.preventDefault();
            onRemove(!removed);
          }}
        >
          {removed ? "Keep" : "Remove"}
        </button>
      </summary>
      {!removed && children}
    </details>
  );
}

function AddEntry({
  label,
  taken,
  onAdd,
}: {
  label: string;
  taken: string[];
  onAdd: (name: string) => void;
}) {
  const [name, setName] = useState("");
  const trimmed = name.trim();
  const error = taken.includes(trimmed) ? "That name is taken." : null;
  return (
    <form
      className="settings-row"
      onSubmit={(e) => {
        e.preventDefault();
        if (!trimmed || error) return;
        onAdd(trimmed);
        setName("");
      }}
    >
      <input
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder="name"
        aria-label={`${label}: its name`}
        size={16}
      />
      <button
        type="submit"
        className="button-small"
        disabled={!trimmed || error !== null}
      >
        {label}
      </button>
      {error && <span className="error-text-small">{error}</span>}
    </form>
  );
}

/** The FSRS parameters in use, which optimizing sets, shown rather than edited. */
function FsrsParameters({ view }: { view: SettingsView }) {
  const parameters = valueAt(view.values, ["fsrs_parameters"]) as
    number[] | undefined;
  const fittedAt = valueAt(view.values, ["fsrs_parameters_fitted_at"]) as
    string | null;
  const count = valueAt(view.values, ["fsrs_parameters_review_count"]) as
    number | null;
  return (
    <p className="field-hint settings-field">
      FSRS parameters:{" "}
      {parameters && parameters.length > 0
        ? fittedAt
          ? `fitted on ${fittedAt} to ${count ?? "?"} reviews.`
          : "set by hand."
        : "the defaults."}{" "}
      Optimize them on the Statistics page.
    </p>
  );
}
