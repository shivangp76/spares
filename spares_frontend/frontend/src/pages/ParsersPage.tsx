import { useEffect, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { createParser, deleteParser, getKnownParserNames, listParsers, updateParser } from '../api/client';
import { NavbarHelp } from '../components/Navbar';
import { ResizableHeader } from '../components/tableColumns';
import { useAuth } from '../hooks/useAuth';
import { useRowKeys } from '../hooks/useRowKeys';
import { useTableWidths } from '../hooks/useTableWidths';
import type { ParserResponse } from '../types/spares';
import { errorMessage, td, th } from '../utils';

// Enough to list every parser on one page
const PARSERS_LIMIT = 1000;
const input: React.CSSProperties = { padding: '6px 10px', fontSize: 14, border: '1px solid var(--border-strong)', borderRadius: 4 };
const smallButton: React.CSSProperties = { padding: '2px 8px', fontSize: 12 };

/** `spares parser list/add/edit/delete`. */
export default function ParsersPage() {
  const { credentials } = useAuth();
  const navigate = useNavigate();
  const [parsers, setParsers] = useState<ParserResponse[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [newName, setNewName] = useState('');
  const [renaming, setRenaming] = useState<{ id: number; name: string } | null>(null);
  // The parsers spares has code for. Null until fetched, or if that failed, so no parser is flagged.
  const [known, setKnown] = useState<string[] | null>(null);
  const widths = useTableWidths('parsers');

  useEffect(() => {
    if (!credentials) { navigate('/login'); return; }
    listParsers(1, PARSERS_LIMIT).then(setParsers, e => setError(errorMessage(e)));
    getKnownParserNames().then(setKnown, () => {});
  }, [credentials, navigate]);

  // The known parsers not added yet, which are the ones worth adding
  const addable = known?.filter(name => !parsers?.some(p => p.name === name)) ?? [];

  async function run(action: () => Promise<string>) {
    try {
      setStatus(await action());
    } catch (e) {
      setStatus(errorMessage(e));
    }
  }

  function add() {
    const name = newName.trim();
    if (!name) return;
    run(async () => {
      const parser = await createParser(name);
      setParsers(prev => (prev ? [...prev, parser] : prev));
      setNewName('');
      return `Parser \`${parser.name}\` added.`;
    });
  }

  function rename() {
    if (!renaming) return;
    const { id, name } = renaming;
    if (!name.trim()) return;
    run(async () => {
      const parser = await updateParser(id, name.trim());
      setParsers(prev => prev?.map(p => (p.id === id ? parser : p)) ?? prev);
      setRenaming(null);
      return `Parser ${id} renamed to \`${parser.name}\`.`;
    });
  }

  function remove(parser: ParserResponse) {
    if (!window.confirm(`Delete parser \`${parser.name}\`?`)) return;
    run(async () => {
      await deleteParser(parser.id);
      setParsers(prev => prev?.filter(p => p.id !== parser.id) ?? prev);
      return `Parser \`${parser.name}\` deleted.`;
    });
  }

  const tableBody = useRef<HTMLTableSectionElement>(null);
  const keys = useRowKeys({
    count: parsers?.length ?? 0,
    body: tableBody,
    edit: { description: 'Rename the highlighted parser', run: i => setRenaming({ id: parsers![i].id, name: parsers![i].name }) },
    remove: { description: 'Delete the highlighted parser, after asking', run: i => remove(parsers![i]) },
  });

  return (
    <div className="page">
      <NavbarHelp help={keys.help} />
      <h2 style={{ marginBottom: 16 }}>Parsers</h2>

      <div className="search-row" style={{ display: 'flex', gap: 8, marginBottom: 16 }}>
        <input
          value={newName}
          onChange={e => setNewName(e.target.value)}
          onKeyDown={e => { if (e.key === 'Enter') add(); }}
          placeholder={addable.length > 0 ? `New parser, e.g. ${addable[0]}` : 'New parser name'}
          list="known-parsers"
          style={{ ...input, flex: 1, maxWidth: 300 }}
        />
        <datalist id="known-parsers">{addable.map(name => <option key={name} value={name} />)}</datalist>
        <button onClick={add} disabled={!newName.trim()} className="touch-target-small">Add</button>
      </div>

      {status && <div style={{ fontSize: 13, color: 'var(--text-secondary)', marginBottom: 12 }}>{status}</div>}
      {error && <div style={{ color: 'var(--error)', marginBottom: 12 }}>{error}</div>}
      {!parsers && !error && <div>Loading…</div>}

      {parsers && (
        <div className="table-scroll">
          <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 14 }}>
            <thead>
              <tr>
                <ResizableHeader widths={widths} column="id" label="ID" />
                <ResizableHeader widths={widths} column="name" label="Name" />
                <th style={th} />
              </tr>
            </thead>
            <tbody ref={tableBody}>
              {parsers.map((parser, i) => (
                <tr key={parser.id} style={i === keys.highlighted ? { background: 'var(--selected)' } : undefined}>
                  <td style={widths.style('id', td)}>{parser.id}</td>
                  <td style={widths.style('name', td)}>
                    {renaming?.id === parser.id ? (
                      <input
                        autoFocus
                        value={renaming.name}
                        onChange={e => setRenaming({ id: parser.id, name: e.target.value })}
                        onKeyDown={e => { if (e.key === 'Enter') rename(); if (e.key === 'Escape') setRenaming(null); }}
                        aria-label="Parser name"
                        style={input}
                      />
                    ) : (
                      <>
                        {parser.name}
                        {known && !known.includes(parser.name) && (
                          <div style={{ fontSize: 12, color: 'var(--warning)' }}>
                            spares has no parser by this name, so notes can't be written for it. Its parsers are {known.join(', ')}.
                          </div>
                        )}
                      </>
                    )}
                  </td>
                  <td style={{ ...td, whiteSpace: 'nowrap', textAlign: 'right' }}>
                    <span style={{ display: 'inline-flex', gap: 4 }}>
                      {renaming?.id === parser.id ? (
                        <>
                          <button onClick={rename} className="touch-target-small" style={smallButton}>Save</button>
                          <button onClick={() => setRenaming(null)} className="touch-target-small" style={smallButton}>Cancel</button>
                        </>
                      ) : (
                        <button onClick={() => setRenaming({ id: parser.id, name: parser.name })} className="touch-target-small" style={smallButton}>Rename</button>
                      )}
                      <button onClick={() => remove(parser)} className="touch-target-small" style={{ ...smallButton, color: 'var(--danger)' }}>Delete</button>
                    </span>
                  </td>
                </tr>
              ))}
              {parsers.length === 0 && (
                <tr>
                  <td colSpan={3} style={{ ...td, color: 'var(--text-muted)', textAlign: 'center' }}>
                    No parsers{addable.length > 0 && `. Add one of ${addable.join(', ')} to write notes in it.`}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
