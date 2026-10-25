import { useCallback, useEffect, useRef, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { ApiError, createTag, deleteTag, listTags, rebuildTag, renameTags, updateTag } from '../api/client';
import Navbar from '../components/Navbar';
import Pagination from '../components/Pagination';
import SearchInput, { type SearchInputError } from '../components/SearchInput';
import { useAuth } from '../hooks/useAuth';
import { useClientPages } from '../hooks/useClientPages';
import { useRowKeys } from '../hooks/useRowKeys';
import { setTagsPageSize, useTagsPageSize } from '../preferences';
import type { TagResponse, UpdateTagRequest } from '../types/spares';
import { errorMessage, td, th } from '../utils';

const PAGE_SIZE = 500;
const input: React.CSSProperties = { padding: '6px 10px', fontSize: 14, border: '1px solid var(--border-strong)', borderRadius: 4 };
const smallButton: React.CSSProperties = { padding: '2px 8px', fontSize: 12 };

async function listAllTags(): Promise<TagResponse[]> {
  const all: TagResponse[] = [];
  for (let page = 1; ; page++) {
    const tags = await listTags(page, PAGE_SIZE);
    all.push(...tags);
    if (tags.length < PAGE_SIZE) return all;
  }
}

/** Where a tag's Review link starts a session: the filtered tag itself, or the cards of notes with the tag. */
function reviewLink(tag: TagResponse): string {
  const params = tag.query !== null
    ? new URLSearchParams({ tagId: String(tag.id) })
    : new URLSearchParams({ query: `tag=${JSON.stringify(tag.name)}` });
  return `/review?${params}`;
}

interface TreeNode {
  children: Map<string, TreeNode>;
}

/** Port of the CLI's `build_tree`: tag names split on `:` into a tree. */
function buildTree(names: string[]): TreeNode {
  const root: TreeNode = { children: new Map() };
  for (const name of names) {
    let current = root;
    for (const part of name.split(':')) {
      let child = current.children.get(part);
      if (!child) {
        child = { children: new Map() };
        current.children.set(part, child);
      }
      current = child;
    }
  }
  return root;
}

const treeToggle: React.CSSProperties = {
  width: 18, padding: 0, marginRight: 2, border: 'none', background: 'none',
  color: 'var(--text-muted)', font: 'inherit', fontSize: 11, cursor: 'pointer',
};

/** How many tags are under `name`, like `math:calculus` under `math` */
function subtagCount(names: string[], name: string): number {
  return names.filter(n => n.startsWith(`${name}:`)).length;
}

/** The last part of a tag's name, which moving it to another parent keeps */
function leafName(name: string): string {
  return name.slice(name.lastIndexOf(':') + 1);
}

/** Where dropping `dragged` on `target` (or on the top level, for `null`) moves it, if anywhere */
function moveTarget(dragged: string, target: string | null): string | null {
  if (target === dragged || target?.startsWith(`${dragged}:`)) return null;
  const to = target === null ? leafName(dragged) : `${target}:${leafName(dragged)}`;
  return to === dragged ? null : to;
}

interface TreeActions {
  /** Full name of the node being renamed */
  renaming: string | null;
  setRenaming: (fullName: string | null) => void;
  /** Full name of the node being dragged */
  dragging: string | null;
  setDragging: (fullName: string | null) => void;
  /** Renames `from` and the tags under it */
  rename: (from: string, to: string) => Promise<void>;
}

interface TagTreeProps {
  node: TreeNode;
  path: string;
  tagsByName: Map<string, TagResponse>;
  /** Full names of the nodes whose children are hidden */
  collapsed: Set<string>;
  onToggle: (fullName: string) => void;
  actions: TreeActions;
}

function RenameInput({ fullName, actions }: { fullName: string; actions: TreeActions }) {
  const [value, setValue] = useState(fullName);
  return (
    <input
      autoFocus
      value={value}
      aria-label={`New name for ${fullName}`}
      onChange={e => setValue(e.target.value)}
      onKeyDown={e => {
        if (e.key === 'Enter') void actions.rename(fullName, value.trim());
        if (e.key === 'Escape') actions.setRenaming(null);
      }}
      onBlur={() => actions.setRenaming(null)}
      style={{ ...input, padding: '1px 6px', minWidth: 240 }}
    />
  );
}

function TagTree({ node, path, tagsByName, collapsed, onToggle, actions }: TagTreeProps) {
  const [dropOn, setDropOn] = useState<string | null>(null);
  const keys = [...node.children.keys()].sort();
  return (
    <ul style={{ listStyle: 'none', paddingLeft: path ? 20 : 0, margin: 0 }}>
      {keys.map(key => {
        const fullName = path ? `${path}:${key}` : key;
        const tag = tagsByName.get(fullName);
        const child = node.children.get(key)!;
        const hasChildren = child.children.size > 0;
        const isCollapsed = collapsed.has(fullName);
        const canDrop = actions.dragging !== null && moveTarget(actions.dragging, fullName) !== null;
        return (
          <li key={key} style={{ fontSize: 14, lineHeight: 1.8 }}>
            <div
              draggable={actions.renaming !== fullName}
              onDragStart={e => {
                e.stopPropagation();
                e.dataTransfer.effectAllowed = 'move';
                e.dataTransfer.setData('text/plain', fullName);
                actions.setDragging(fullName);
              }}
              onDragEnd={() => { actions.setDragging(null); setDropOn(null); }}
              onDragOver={e => {
                if (!canDrop) return;
                e.preventDefault();
                e.dataTransfer.dropEffect = 'move';
                setDropOn(fullName);
              }}
              onDragLeave={() => setDropOn(prev => (prev === fullName ? null : prev))}
              onDrop={e => {
                e.preventDefault();
                setDropOn(null);
                const dragged = actions.dragging;
                const to = dragged && moveTarget(dragged, fullName);
                actions.setDragging(null);
                if (dragged && to) void actions.rename(dragged, to);
              }}
              style={{
                display: 'flex', alignItems: 'center', borderRadius: 4,
                cursor: actions.renaming === fullName ? undefined : 'grab',
                outline: dropOn === fullName ? '2px dashed var(--accent)' : undefined,
              }}
            >
              {hasChildren ? (
                <button
                  onClick={() => onToggle(fullName)}
                  aria-expanded={!isCollapsed}
                  aria-label={`${isCollapsed ? 'Expand' : 'Collapse'} ${fullName}`}
                  className="tap-area"
                  style={treeToggle}
                >
                  {isCollapsed ? '▶' : '▼'}
                </button>
              ) : (
                <span style={{ display: 'inline-block', width: 18, marginRight: 2 }} />
              )}
              {actions.renaming === fullName ? (
                <RenameInput fullName={fullName} actions={actions} />
              ) : (
                <span style={{ color: tag ? undefined : 'var(--text-faint)' }}>{key || '(empty)'}</span>
              )}
              <span style={{ fontSize: 12, color: 'var(--text-muted)', marginLeft: 8 }}>
                {tag && <>#{tag.id}{tag.query !== null && ' · filtered'} · <Link to={reviewLink(tag)}>Review</Link> · </>}
                <button
                  onClick={() => actions.setRenaming(fullName)}
                  title={hasChildren ? 'Rename, with the tags under it' : 'Rename'}
                  style={{ ...smallButton, padding: 0, border: 'none', background: 'none', color: 'var(--link)', textDecoration: 'underline', cursor: 'pointer' }}
                >
                  Rename
                </button>
              </span>
            </div>
            {hasChildren && !isCollapsed && (
              <TagTree node={child} path={fullName} tagsByName={tagsByName} collapsed={collapsed} onToggle={onToggle} actions={actions} />
            )}
          </li>
        );
      })}
    </ul>
  );
}

/** Where a dragged tag is dropped to move it to the top level. Always shown, so that starting a
    drag doesn't move the tree out from under the pointer. */
function TopLevelDropZone({ actions }: { actions: TreeActions }) {
  const [over, setOver] = useState(false);
  const to = actions.dragging && moveTarget(actions.dragging, null);
  return (
    <div
      onDragOver={e => {
        if (!to) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={e => {
        e.preventDefault();
        setOver(false);
        const dragged = actions.dragging;
        actions.setDragging(null);
        if (dragged && to) void actions.rename(dragged, to);
      }}
      style={{
        border: `2px dashed ${over ? 'var(--accent)' : 'var(--border)'}`,
        borderRadius: 6, padding: 8, marginBottom: 8, fontSize: 13, color: to ? 'var(--text-secondary)' : 'var(--text-faint)', textAlign: 'center',
      }}
    >
      {to ? <>Drop here to move to the top level, as <code>{to}</code></> : 'Drag a tag here to move it to the top level'}
    </div>
  );
}

/** Creates a tag, or edits one, sending only the fields that changed (as `tag edit` does). */
function TagForm({ tag, subtags, onSaved, onCancel }: { tag: TagResponse | null; subtags: number; onSaved: (tag: TagResponse) => void; onCancel: () => void }) {
  const [name, setName] = useState(tag?.name ?? '');
  const [description, setDescription] = useState(tag?.description ?? '');
  const [query, setQuery] = useState(tag?.query ?? '');
  const [autoDelete, setAutoDelete] = useState(tag?.auto_delete ?? true);
  const [reschedule, setReschedule] = useState(tag?.reschedule ?? true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [queryError, setQueryError] = useState<SearchInputError | null>(null);

  async function save() {
    if (!name.trim()) { setError('Name is empty'); return; }
    setSaving(true);
    setError(null);
    setQueryError(null);
    const newQuery = query.trim() || null;
    try {
      if (tag) {
        const req: UpdateTagRequest = { tag_to_modify: { Id: tag.id } };
        if (name !== tag.name) req.name = name;
        if (description !== tag.description) req.description = description;
        if (newQuery !== tag.query) req.query = newQuery;
        if (autoDelete !== tag.auto_delete) req.auto_delete = autoDelete;
        if (reschedule !== tag.reschedule) req.reschedule = reschedule;
        onSaved(Object.keys(req).length > 1 ? await updateTag(req) : tag);
      } else {
        onSaved(await createTag({ name, description, query: newQuery, auto_delete: autoDelete, reschedule }));
      }
    } catch (e) {
      setError(errorMessage(e));
      // Only the query is searched, so a place in an error is a place in it
      if (newQuery && e instanceof ApiError && e.span) setQueryError({ query: newQuery, span: e.span });
    } finally {
      setSaving(false);
    }
  }

  // As in the parsers' rename box
  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === 'Enter') void save();
    if (e.key === 'Escape') onCancel();
  }

  return (
    <div style={{ border: '1px solid var(--border)', borderRadius: 6, padding: 16, marginBottom: 16, background: 'var(--surface)' }}>
      <h3 style={{ marginTop: 0, fontSize: 16 }}>{tag ? `Edit tag #${tag.id}` : 'New tag'}</h3>
      <div className="label-grid" style={{ display: 'grid', gridTemplateColumns: 'auto 1fr', gap: '8px 12px', alignItems: 'center', fontSize: 14 }}>
        <label htmlFor="tag-name">Name</label>
        <input id="tag-name" autoFocus value={name} onChange={e => setName(e.target.value)} onKeyDown={handleKeyDown} placeholder="e.g. math:algebra" style={input} />
        {tag && name !== tag.name && subtags > 0 && (
          <>
            <span />
            <span style={{ fontSize: 12, color: 'var(--text-muted)' }}>
              Also renames the {subtags === 1 ? 'tag' : `${subtags} tags`} under it, like <code>{name}:…</code>
            </span>
          </>
        )}
        <label htmlFor="tag-description">Description</label>
        <input id="tag-description" value={description} onChange={e => setDescription(e.target.value)} onKeyDown={handleKeyDown} style={input} />
        <label htmlFor="tag-query">Query</label>
        <SearchInput
          id="tag-query"
          value={query}
          onChange={setQuery}
          onSubmit={() => { void save(); }}
          placeholder="Makes a filtered tag; leave empty for a regular tag"
          error={queryError}
          style={{ maxWidth: 'none' }}
          inputStyle={input}
        />
        <span />
        <label>
          <input type="checkbox" checked={autoDelete} onChange={e => setAutoDelete(e.target.checked)} style={{ marginRight: 6 }} />
          Delete automatically when no notes have it
        </label>
        {query.trim() && (
          <>
            <span />
            <label title="Off to cram: cards are shown until the scheduler lets them go (with FSRS, when rated Good twice or Easy once), without changing when they are next due">
              <input type="checkbox" checked={reschedule} onChange={e => setReschedule(e.target.checked)} style={{ marginRight: 6 }} />
              Reschedule cards rated while studying this tag
            </label>
          </>
        )}
      </div>
      <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginTop: 12 }}>
        <button onClick={save} disabled={saving} className="touch-target-small">{saving ? 'Saving…' : tag ? 'Save' : 'Create'}</button>
        <button onClick={onCancel} disabled={saving} className="touch-target-small">Cancel</button>
        {error && <span style={{ color: 'var(--error)', fontSize: 13, whiteSpace: 'pre-wrap', fontFamily: 'monospace' }}>{error}</span>}
      </div>
    </div>
  );
}

export default function TagsPage() {
  const { credentials, logout } = useAuth();
  const navigate = useNavigate();
  const [tags, setTags] = useState<TagResponse[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [filter, setFilter] = useState('');
  const [view, setView] = useState<'table' | 'tree'>('table');
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  // `null` for a new tag
  const [editing, setEditing] = useState<{ tag: TagResponse | null } | null>(null);

  const load = useCallback(() => {
    listAllTags().then(setTags, e => setError(errorMessage(e)));
  }, []);

  useEffect(() => {
    if (!credentials) { navigate('/login'); return; }
    load();
  }, [credentials, navigate, load]);

  async function remove(tag: TagResponse) {
    if (!window.confirm(`Delete tag \`${tag.name}\`?`)) return;
    try {
      await deleteTag(tag.id);
      setTags(prev => prev?.filter(t => t.id !== tag.id) ?? prev);
      setStatus(`Tag \`${tag.name}\` deleted.`);
    } catch (e) {
      setStatus(errorMessage(e));
    }
  }

  async function rebuild(tag: TagResponse) {
    try {
      await rebuildTag(tag.id);
      setStatus(`Filtered tag \`${tag.name}\` rebuilt.`);
    } catch (e) {
      setStatus(errorMessage(e));
    }
  }

  const [renaming, setRenaming] = useState<string | null>(null);
  const [dragging, setDragging] = useState<string | null>(null);

  async function rename(from: string, to: string) {
    setRenaming(null);
    if (!to || to === from) return;
    try {
      await renameTags(from, to);
      setStatus(`Renamed \`${from}\` to \`${to}\`.`);
    } catch (e) {
      setStatus(errorMessage(e));
    }
    load();
  }

  function onSaved(saved: TagResponse) {
    // A rename also renames the tags under it, and a merge deletes the tag, so read them again.
    if (editing?.tag && (saved.id !== editing.tag.id || saved.name !== editing.tag.name)) {
      load();
      setStatus(`Tag \`${saved.name}\` saved.`);
      setEditing(null);
      return;
    }
    setTags(prev => {
      if (!prev) return prev;
      return prev.some(t => t.id === saved.id) ? prev.map(t => (t.id === saved.id ? saved : t)) : [...prev, saved];
    });
    setStatus(`Tag \`${saved.name}\` saved.`);
    setEditing(null);
  }

  function toggleCollapsed(fullName: string) {
    setCollapsed(prev => {
      const next = new Set(prev);
      if (!next.delete(fullName)) next.add(fullName);
      return next;
    });
  }

  const treeActions: TreeActions = { renaming, setRenaming, dragging, setDragging, rename };

  const needle = filter.trim().toLowerCase();
  const found = (tags ?? []).filter(t => !needle || t.name.toLowerCase().includes(needle));
  // Only the table is split into pages, as the tree's branches would be split across them
  const pages = useClientPages(found, useTagsPageSize(), setTagsPageSize);
  const shown = pages.shown;

  const tableBody = useRef<HTMLTableSectionElement>(null);
  const filterInput = useRef<HTMLInputElement>(null);
  const keys = useRowKeys({
    // The tree has no rows to highlight
    count: view === 'table' ? shown.length : 0,
    body: tableBody,
    edit: { description: 'Edit the highlighted tag', run: i => setEditing({ tag: shown[i] }) },
    remove: { description: 'Delete the highlighted tag, after asking', run: i => void remove(shown[i]) },
    filter: filterInput,
    prevPage: view === 'table' ? pages.pagination.prevPage : null,
    nextPage: view === 'table' ? pages.pagination.nextPage : null,
  });

  return (
    <div className="page" style={{ maxWidth: 'max(1000px, var(--page-max-width))' }}>
      <div className="page-navbar">
        <Navbar onLogout={logout} help={keys.help} />
      </div>
      <h2 style={{ marginBottom: 16 }}>Tags</h2>

      <div className="search-row" style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', marginBottom: 16 }}>
        <input ref={filterInput} value={filter} onChange={e => { setFilter(e.target.value); pages.pagination.goToPage(1); }} placeholder="Filter by name…" style={{ ...input, flex: 1, maxWidth: 300 }} />
        <label style={{ fontSize: 14 }}>
          <input type="radio" name="tag-view" checked={view === 'table'} onChange={() => setView('table')} style={{ marginRight: 4 }} />Table
        </label>
        <label style={{ fontSize: 14 }}>
          <input type="radio" name="tag-view" checked={view === 'tree'} onChange={() => setView('tree')} style={{ marginRight: 4 }} />Tree
        </label>
        <button onClick={() => setEditing({ tag: null })} className="touch-target-small" style={{ marginLeft: 'auto' }}>New tag</button>
      </div>

      {editing && (
        <TagForm
          key={editing.tag?.id ?? 'new'}
          tag={editing.tag}
          subtags={editing.tag ? subtagCount((tags ?? []).map(t => t.name), editing.tag.name) : 0}
          onSaved={onSaved}
          onCancel={() => setEditing(null)}
        />
      )}
      {status && <div style={{ fontSize: 13, color: 'var(--text-secondary)', marginBottom: 12 }}>{status}</div>}
      {error && <pre style={{ color: 'var(--error)', marginBottom: 12, marginTop: 0, whiteSpace: 'pre-wrap' }}>{error}</pre>}
      {!tags && !error && <div>Loading…</div>}

      {tags && view === 'tree' && (
        <>
          <p style={{ fontSize: 13, color: 'var(--text-muted)', marginTop: 0 }}>
            Drag a tag onto another to move it there. Renaming or moving a tag takes the tags under it along.
          </p>
          <TopLevelDropZone actions={treeActions} />
          <TagTree
            node={buildTree(found.map(t => t.name))}
            path=""
            tagsByName={new Map(tags.map(t => [t.name, t]))}
            collapsed={collapsed}
            onToggle={toggleCollapsed}
            actions={treeActions}
          />
        </>
      )}

      {tags && view === 'table' && (
        <>
          <div className="table-scroll">
            <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 14 }}>
              <thead>
                <tr>
                  <th style={th} className="hide-narrow">ID</th>
                  <th style={th}>Name</th>
                  <th style={th} className="hide-narrow">Description</th>
                  <th style={th} className="hide-narrow">Query</th>
                  <th style={th} className="hide-narrow">Auto delete</th>
                  <th style={th} />
                </tr>
              </thead>
              <tbody ref={tableBody}>
                {shown.map((tag, i) => (
                  <tr key={tag.id} style={i === keys.highlighted ? { background: 'var(--selected)' } : undefined}>
                    <td style={td} className="hide-narrow">{tag.id}</td>
                    <td style={{ ...td, overflowWrap: 'anywhere' }}>{tag.name}</td>
                    <td style={td} className="hide-narrow">{tag.description || '—'}</td>
                    <td style={td} className="hide-narrow">{tag.query !== null ? <><code>{tag.query}</code>{!tag.reschedule && ' (no rescheduling)'}</> : '—'}</td>
                    <td style={td} className="hide-narrow">{tag.auto_delete ? 'Yes' : 'No'}</td>
                    <td style={{ ...td, whiteSpace: 'nowrap' }}>
                      <span style={{ display: 'inline-flex', gap: 4 }}>
                        <Link to={reviewLink(tag)} style={{ fontSize: 12, alignSelf: 'center', marginRight: 4 }}>Review</Link>
                        <button onClick={() => setEditing({ tag })} className="touch-target-small" style={smallButton}>Edit</button>
                        {tag.query !== null && <button onClick={() => rebuild(tag)} className="touch-target-small" style={smallButton}>Rebuild</button>}
                        <button onClick={() => remove(tag)} className="touch-target-small" style={{ ...smallButton, color: 'var(--danger)' }}>Delete</button>
                      </span>
                    </td>
                  </tr>
                ))}
                {shown.length === 0 && (
                  <tr><td colSpan={6} style={{ ...td, color: 'var(--text-muted)', textAlign: 'center' }}>No tags found</td></tr>
                )}
              </tbody>
            </table>
          </div>
          <Pagination {...pages.pagination} />
        </>
      )}
    </div>
  );
}
