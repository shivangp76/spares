import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import CodeMirror, {
  type Extension,
  type ReactCodeMirrorRef,
} from "@uiw/react-codemirror";
import { setDiagnostics } from "@codemirror/lint";
import {
  getSettings,
  replaceSettings,
  SETTINGS_CHANGED_SINCE,
  updateSettings,
} from "../api/client";
import { errorLine, type Draft, draftValues } from "../collectionSettings";
import { useEditorSetup } from "../hooks/useEditorSetup";
import { noteDiagnostics } from "../noteDiagnostics";
import { useResolvedTheme } from "../theme";
import type { SettingsChangeResponse, SettingsView } from "../types/spares";
import { errorMessage } from "../utils";
import { useVimCommands } from "../vimCommands";
import ActionResult, { type ActionOutcome } from "./ActionResult";
import SettingsForm from "./SettingsForm";

type Mode = "form" | "toml";

const cmStyle = { border: "1px solid var(--border-subtle)", borderRadius: 4 };

/** The TOML language, loaded when the editor first opens. */
async function loadToml(): Promise<Extension> {
  const [{ StreamLanguage }, { toml }] = await Promise.all([
    import("@codemirror/language"),
    import("@codemirror/legacy-modes/mode/toml"),
  ]);
  return StreamLanguage.define(toml);
}

function describeChange(response: SettingsChangeResponse): string {
  const keys = response.changed_keys;
  if (keys.length === 0) return "Nothing changed.";
  if (keys.length <= 3)
    return `Saved ${keys.map((key) => `${key}`).join(", ")}.`;
  return `Saved ${keys.length} settings.`;
}

/**
 * The collection's settings, which every device shares and syncs: a form for the common ones,
 * and the whole TOML text, comments included. This machine's settings stay in the host's
 * config.toml, shown below. Each save is a change that can be undone.
 */
export default function CollectionSettings() {
  const [view, setView] = useState<SettingsView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [mode, setMode] = useState<Mode>("form");
  const [outcome, setOutcome] = useState<ActionOutcome | null>(null);

  const reload = useCallback(async () => {
    try {
      setView(await getSettings());
      setLoadError(null);
    } catch (e) {
      setLoadError(errorMessage(e));
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    getSettings().then(
      (settings) => {
        if (!cancelled) setView(settings);
      },
      (e: unknown) => {
        if (!cancelled) setLoadError(errorMessage(e));
      },
    );
    return () => {
      cancelled = true;
    };
  }, []);

  function saved(response: SettingsChangeResponse) {
    setView(response.settings);
    setOutcome({
      message: describeChange(response),
      eventIds: response.event_id === null ? [] : [response.event_id],
    });
  }

  return (
    <fieldset className="settings-section">
      <legend className="section-label">Collection</legend>
      <p className="field-hint" style={{ marginTop: 0 }}>
        Shared by every device: changes sync, and can be made offline on a
        device that keeps a copy. Also <code>spares config edit</code>.
      </p>
      <div className="settings-choices settings-row" role="tablist">
        {(
          [
            ["form", "Common settings"],
            ["toml", "All settings (TOML)"],
          ] as const
        ).map(([value, label]) => (
          <label key={value} className="checkbox-label">
            <input
              type="radio"
              name="collection-settings-mode"
              checked={mode === value}
              onChange={() => setMode(value)}
            />
            {label}
          </label>
        ))}
      </div>
      {loadError && <p className="error-text">{loadError}</p>}
      {!view && !loadError && <p className="field-hint">Loading…</p>}
      {view &&
        (mode === "form" ? (
          <SettingsForm
            key={view.text}
            view={view}
            onSave={async (drafts: Draft[]) =>
              saved(await updateSettings(draftValues(drafts)))
            }
          />
        ) : (
          <TomlEditor
            key={view.text}
            view={view}
            onSaved={saved}
            onReload={() => void reload()}
          />
        ))}
      {outcome && (
        <ActionResult
          key={outcome.eventIds.join(",") + outcome.message}
          outcome={outcome}
          onReverted={() => void reload()}
        />
      )}
      {view && view.local.trim() !== "" && (
        <details style={{ fontSize: "var(--font-sm)", marginTop: 12 }}>
          <summary style={{ cursor: "pointer" }}>
            This machine&apos;s settings
          </summary>
          <p className="field-hint">
            {view.local_keys.map((key) => `${key}`).join(", ")} are the
            host&apos;s own, in its config.toml, and change only there.
          </p>
          <pre className="settings-local">{view.local.trim()}</pre>
        </details>
      )}
    </fieldset>
  );
}

/** The settings as TOML, saved whole, only over the text it was opened from. */
function TomlEditor({
  view,
  onSaved,
  onReload,
}: {
  view: SettingsView;
  onSaved: (response: SettingsChangeResponse) => void;
  onReload: () => void;
}) {
  const theme = useResolvedTheme();
  const { extensions, dataSetup } = useEditorSetup();
  const [text, setText] = useState(view.text);
  const [error, setError] = useState<string | null>(null);
  const [changedSince, setChangedSince] = useState(false);
  const [saving, setSaving] = useState(false);
  const [language, setLanguage] = useState<Extension | null>(null);
  const editorRef = useRef<ReactCodeMirrorRef>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const dirty = text !== view.text;

  useEffect(() => {
    let cancelled = false;
    loadToml().then(
      (extension) => {
        if (!cancelled) setLanguage(extension);
      },
      (e: unknown) =>
        console.error("Failed to load the TOML editor language", e),
    );
    return () => {
      cancelled = true;
    };
  }, []);

  const allExtensions = useMemo(
    // The marked error line goes with the next edit
    () => [...extensions, ...noteDiagnostics, ...(language ? [language] : [])],
    [extensions, language],
  );

  function markError(message: string | null) {
    const editor = editorRef.current?.view;
    if (!editor) return;
    const line = message === null ? null : errorLine(message);
    const diagnostics =
      line !== null && line >= 1 && line <= editor.state.doc.lines
        ? [
            {
              from: editor.state.doc.line(line).from,
              to: editor.state.doc.line(line).to,
              severity: "error" as const,
              message: message!,
            },
          ]
        : [];
    editor.dispatch(setDiagnostics(editor.state, diagnostics));
    if (diagnostics.length)
      editor.dispatch({
        selection: { anchor: diagnostics[0].from },
        scrollIntoView: true,
      });
  }

  async function save() {
    if (!dirty || saving) return;
    setSaving(true);
    try {
      onSaved(await replaceSettings(text, view.text));
    } catch (e) {
      const message = errorMessage(e);
      setError(message);
      setChangedSince(message.includes(SETTINGS_CHANGED_SINCE));
      markError(message);
    } finally {
      setSaving(false);
    }
  }

  function revert() {
    setText(view.text);
    setError(null);
    markError(null);
  }

  useVimCommands(rootRef, ({ command, force, warn }) => {
    if (command === "write" || command === "writeQuit" || command === "exit")
      void save();
    else if (command === "edit" && force) revert();
    else warn(`E492: Not an editor command here`);
  });

  return (
    <div ref={rootRef} className="settings-row" style={{ display: "block" }}>
      <CodeMirror
        ref={editorRef}
        theme={theme}
        value={text}
        onChange={(value) => {
          setText(value);
          setError(null);
        }}
        extensions={allExtensions}
        basicSetup={dataSetup}
        minHeight="240px"
        maxHeight="70vh"
        style={cmStyle}
        aria-label="The collection's settings, as TOML"
      />
      <p className="field-hint">
        Every setting is documented on <code>SparesExternalConfig</code> in{" "}
        <code>spares_core::config</code>. Settings left out take their defaults.
        With Vim keybindings, :w saves and :e! goes back to the saved text.
      </p>
      {error && <p className="error-text-small">{error}</p>}
      <div className="settings-row">
        <button onClick={() => void save()} disabled={!dirty || saving}>
          {saving ? "Saving…" : "Save"}
        </button>
        <button onClick={revert} disabled={!dirty || saving}>
          Discard changes
        </button>
        {changedSince && (
          <button
            onClick={() => {
              if (
                window.confirm(
                  "Load the settings as they are now? Your changes here are lost.",
                )
              )
                onReload();
            }}
          >
            Load the current settings
          </button>
        )}
      </div>
    </div>
  );
}
