import { useCallback, useEffect, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { createTag, deleteTag, listTags, rebuildTag, updateTag } from '../api/client';
import Navbar from '../components/Navbar';
import { useAuth } from '../hooks/useAuth';
import type { TagResponse, UpdateTagRequest } from '../types/spares';
import { td, th } from '../utils';

const PAGE_SIZE = 500;
const input: React.CSSProperties = { padding: '6px 10px', fontSize: 14, border: '1px solid #ccc', borderRadius: 4 };
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

function TagTree({ node, path, tagsByName }: { node: TreeNode; path: string; tagsByName: Map<string, TagResponse> }) {
  const keys = [...node.children.keys()].sort();
  return (
    <ul style={{ listStyle: 'none', paddingLeft: path ? 20 : 0, margin: 0 }}>
      {keys.map(key => {
        const fullName = path ? `${path}:${key}` : key;
        const tag = tagsByName.get(fullName);
        return (
          <li key={key} style={{ fontSize: 14, lineHeight: 1.8 }}>
            <span style={{ color: tag ? undefined : '#999' }}>{key || '(empty)'}</span>
            {tag && (
              <span style={{ fontSize: 12, color: '#888', marginLeft: 8 }}>
                #{tag.id}{tag.query !== null && ' · filtered'} · <Link to={reviewLink(tag)}>Review</Link>
              </span>
            )}
            <TagTree node={node.children.get(key)!} path={fullName} tagsByName={tagsByName} />
          </li>
        );
      })}
    </ul>
  );
}

/** Creates a tag, or edits one, sending only the fields that changed (as `tag edit` does). */
function TagForm({ tag, onSaved, onCancel }: { tag: TagResponse | null; onSaved: (tag: TagResponse) => void; onCancel: () => void }) {
  const [name, setName] = useState(tag?.name ?? '');
  const [description, setDescription] = useState(tag?.description ?? '');
  const [query, setQuery] = useState(tag?.query ?? '');
  const [autoDelete, setAutoDelete] = useState(tag?.auto_delete ?? true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function save() {
    if (!name.trim()) { setError('Name is empty'); return; }
    setSaving(true);
    setError(null);
    const newQuery = query.trim() || null;
    try {
      if (tag) {
        const req: UpdateTagRequest = { tag_to_modify: { Id: tag.id } };
        if (name !== tag.name) req.name = name;
        if (description !== tag.description) req.description = description;
        if (newQuery !== tag.query) req.query = newQuery;
        if (autoDelete !== tag.auto_delete) req.auto_delete = autoDelete;
        onSaved(Object.keys(req).length > 1 ? await updateTag(req) : tag);
      } else {
        onSaved(await createTag({ name, description, query: newQuery, auto_delete: autoDelete }));
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div style={{ border: '1px solid #ddd', borderRadius: 6, padding: 16, marginBottom: 16, background: '#fafafa' }}>
      <h3 style={{ marginTop: 0, fontSize: 16 }}>{tag ? `Edit tag #${tag.id}` : 'New tag'}</h3>
      <div style={{ display: 'grid', gridTemplateColumns: 'auto 1fr', gap: '8px 12px', alignItems: 'center', fontSize: 14 }}>
        <label htmlFor="tag-name">Name</label>
        <input id="tag-name" value={name} onChange={e => setName(e.target.value)} placeholder="e.g. math:algebra" style={input} />
        <label htmlFor="tag-description">Description</label>
        <input id="tag-description" value={description} onChange={e => setDescription(e.target.value)} style={input} />
        <label htmlFor="tag-query">Query</label>
        <input id="tag-query" value={query} onChange={e => setQuery(e.target.value)} placeholder="Makes a filtered tag; leave empty for a regular tag" style={input} />
        <span />
        <label>
          <input type="checkbox" checked={autoDelete} onChange={e => setAutoDelete(e.target.checked)} style={{ marginRight: 6 }} />
          Delete automatically when no notes have it
        </label>
      </div>
      <div style={{ display: 'flex', gap: 8, alignItems: 'center', marginTop: 12 }}>
        <button onClick={save} disabled={saving}>{saving ? 'Saving…' : tag ? 'Save' : 'Create'}</button>
        <button onClick={onCancel} disabled={saving}>Cancel</button>
        {error && <span style={{ color: 'red', fontSize: 13 }}>{error}</span>}
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
  // `null` for a new tag
  const [editing, setEditing] = useState<{ tag: TagResponse | null } | null>(null);

  const load = useCallback(() => {
    listAllTags().then(setTags, e => setError(String(e)));
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
      setStatus(String(e));
    }
  }

  async function rebuild(tag: TagResponse) {
    try {
      await rebuildTag(tag.id);
      setStatus(`Filtered tag \`${tag.name}\` rebuilt.`);
    } catch (e) {
      setStatus(String(e));
    }
  }

  function onSaved(saved: TagResponse) {
    setTags(prev => {
      if (!prev) return prev;
      return prev.some(t => t.id === saved.id) ? prev.map(t => (t.id === saved.id ? saved : t)) : [...prev, saved];
    });
    setStatus(`Tag \`${saved.name}\` saved.`);
    setEditing(null);
  }

  const needle = filter.trim().toLowerCase();
  const shown = (tags ?? []).filter(t => !needle || t.name.toLowerCase().includes(needle));

  return (
    <div style={{ maxWidth: 1000, margin: '0 auto', padding: 24 }}>
      <div style={{ maxWidth: 800, margin: '0 auto' }}>
        <Navbar onLogout={logout} />
      </div>
      <h2 style={{ marginBottom: 16 }}>Tags</h2>

      <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap', marginBottom: 16 }}>
        <input value={filter} onChange={e => setFilter(e.target.value)} placeholder="Filter by name…" style={{ ...input, flex: 1, maxWidth: 300 }} />
        <label style={{ fontSize: 14 }}>
          <input type="radio" name="tag-view" checked={view === 'table'} onChange={() => setView('table')} style={{ marginRight: 4 }} />Table
        </label>
        <label style={{ fontSize: 14 }}>
          <input type="radio" name="tag-view" checked={view === 'tree'} onChange={() => setView('tree')} style={{ marginRight: 4 }} />Tree
        </label>
        <button onClick={() => setEditing({ tag: null })} style={{ marginLeft: 'auto' }}>New tag</button>
      </div>

      {editing && <TagForm key={editing.tag?.id ?? 'new'} tag={editing.tag} onSaved={onSaved} onCancel={() => setEditing(null)} />}
      {status && <div style={{ fontSize: 13, color: '#555', marginBottom: 12 }}>{status}</div>}
      {error && <div style={{ color: 'red', marginBottom: 12 }}>Error: {error}</div>}
      {!tags && !error && <div>Loading…</div>}

      {tags && view === 'tree' && (
        <TagTree node={buildTree(shown.map(t => t.name))} path="" tagsByName={new Map(tags.map(t => [t.name, t]))} />
      )}

      {tags && view === 'table' && (
        <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 14 }}>
          <thead>
            <tr>
              <th style={th}>ID</th>
              <th style={th}>Name</th>
              <th style={th}>Description</th>
              <th style={th}>Query</th>
              <th style={th}>Auto delete</th>
              <th style={th} />
            </tr>
          </thead>
          <tbody>
            {shown.map(tag => (
              <tr key={tag.id}>
                <td style={td}>{tag.id}</td>
                <td style={td}>{tag.name}</td>
                <td style={td}>{tag.description || '—'}</td>
                <td style={td}>{tag.query !== null ? <code>{tag.query}</code> : '—'}</td>
                <td style={td}>{tag.auto_delete ? 'Yes' : 'No'}</td>
                <td style={{ ...td, whiteSpace: 'nowrap' }}>
                  <span style={{ display: 'inline-flex', gap: 4 }}>
                    <Link to={reviewLink(tag)} style={{ fontSize: 12, alignSelf: 'center', marginRight: 4 }}>Review</Link>
                    <button onClick={() => setEditing({ tag })} style={smallButton}>Edit</button>
                    {tag.query !== null && <button onClick={() => rebuild(tag)} style={smallButton}>Rebuild</button>}
                    <button onClick={() => remove(tag)} style={{ ...smallButton, color: '#b00' }}>Delete</button>
                  </span>
                </td>
              </tr>
            ))}
            {shown.length === 0 && (
              <tr><td colSpan={6} style={{ ...td, color: '#888', textAlign: 'center' }}>No tags found</td></tr>
            )}
          </tbody>
        </table>
      )}
    </div>
  );
}
