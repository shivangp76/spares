import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { STUDY_COLORS, bounds, forceLayout, layeredLayout, lineage, nodeLabel, studyState, tagColor, type ForceNode, type Point } from '../graphLayout';
import type { GraphEdge, GraphNode } from '../types/spares';
import { formatNoteId } from '../utils';

export type GraphLayout = 'force' | 'layered';
export type GraphColor = 'scope' | 'study' | 'tag';

/** Zoomed out further than this, only the notes in focus are named. */
const LABEL_ZOOM = 0.7;
const MIN_ZOOM = 0.05;
const MAX_ZOOM = 6;
/** The most a few notes are zoomed in to fit them. */
const MAX_FIT_ZOOM = 1.5;
/** How far the pointer can move and still click rather than drag. */
const CLICK_SLOP = 4;

function nodeColor(node: GraphNode, colorBy: GraphColor): string {
  switch (colorBy) {
    case 'scope': return node.in_query ? 'var(--accent)' : 'var(--text-faint)';
    case 'study': return STUDY_COLORS[studyState(node.cards)].color;
    case 'tag': return tagColor(node.tags[0]);
  }
}

function radius(node: GraphNode): number {
  const { new: n, learning, review, suspended } = node.cards;
  return Math.min(16, 4 + 1.5 * Math.sqrt(n + learning + review + suspended));
}

interface Transform { x: number; y: number; k: number }

interface Props {
  nodes: GraphNode[];
  edges: GraphEdge[];
  layout: GraphLayout;
  colorBy: GraphColor;
  selected: number | null;
  onSelect: (id: number | null) => void;
  /** Notes to ring, such as those a search in the graph finds. */
  marked: Set<number>;
  /** Moves the view to this note when it changes. */
  centerOn: number | null;
  height: number | string;
}

/**
 * Notes as circles, with an arrow from each note to the notes that build on it. Drag the
 * background to pan, the wheel to zoom, and in the force layout, drag a note to move it.
 * Pointing at a note shows the notes it builds on and those that build on it.
 */
export default function NoteGraph({ nodes, edges, layout, colorBy, selected, onSelect, marked, centerOn, height }: Props) {
  const svg = useRef<SVGSVGElement>(null);
  const [size, setSize] = useState({ width: 800, height: 600 });
  const [view, setView] = useState<Transform>({ x: 400, y: 300, k: 1 });
  const [hovered, setHovered] = useState<number | null>(null);
  // Bumped on each tick of a drag in the force layout, to redraw the notes it moved
  const [, setTicks] = useState(0);
  const drag = useRef<{ kind: 'pan' | 'node'; id?: number; startX: number; startY: number; moved: boolean; view: Transform } | null>(null);

  const placed = useMemo(() => {
    const ids = nodes.map(n => n.id);
    if (layout === 'layered') return { at: layeredLayout(ids, edges), simulation: null };
    const forceNodes: ForceNode[] = nodes.map(n => ({ id: n.id, radius: radius(n) }));
    const simulation = forceLayout(forceNodes, edges);
    return { at: new Map<number, Point>(forceNodes.map(n => [n.id, n as Point])), simulation };
  }, [nodes, edges, layout]);
  useEffect(() => () => { placed.simulation?.stop(); }, [placed]);

  useLayoutEffect(() => {
    const el = svg.current;
    if (!el) return;
    const observer = new ResizeObserver(() => setSize({ width: el.clientWidth, height: el.clientHeight }));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  /** Fits every note in view. */
  const fit = () => {
    const box = bounds(placed.at.values());
    const k = Math.min(MAX_FIT_ZOOM, Math.max(MIN_ZOOM, Math.min(size.width / box.width, size.height / box.height)));
    setView({ k, x: size.width / 2 - (box.x + box.width / 2) * k, y: size.height / 2 - (box.y + box.height / 2) * k });
  };
  // Refit only for a new layout or size, not after panning
  // eslint-disable-next-line react-hooks/set-state-in-effect, react-hooks/exhaustive-deps -- fits the view to the notes laid out
  useEffect(() => fit(), [placed, size.width, size.height]);

  useEffect(() => {
    const at = centerOn === null ? undefined : placed.at.get(centerOn);
    if (!at) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- moves the view when asked to
    setView(v => {
      const k = Math.max(v.k, 1);
      return { k, x: size.width / 2 - at.x * k, y: size.height / 2 - at.y * k };
    });
  }, [centerOn, placed, size.width, size.height]);

  // Not `onWheel`, which React makes passive, so the page would scroll too
  useEffect(() => {
    const el = svg.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      const px = e.clientX - rect.left, py = e.clientY - rect.top;
      setView(v => {
        const k = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, v.k * Math.exp(-e.deltaY * 0.002)));
        return { k, x: px - (px - v.x) * (k / v.k), y: py - (py - v.y) * (k / v.k) };
      });
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, []);

  const zoomBy = (factor: number) => setView(v => {
    const k = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, v.k * factor));
    const cx = size.width / 2, cy = size.height / 2;
    return { k, x: cx - (cx - v.x) * (k / v.k), y: cy - (cy - v.y) * (k / v.k) };
  });

  function onPointerDown(e: React.PointerEvent, id?: number) {
    if (e.button !== 0) return;
    e.stopPropagation();
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    drag.current = { kind: id === undefined ? 'pan' : 'node', id, startX: e.clientX, startY: e.clientY, moved: false, view };
    if (id !== undefined && placed.simulation) {
      placed.simulation.on('tick', () => setTicks(t => t + 1)).alphaTarget(0.3).restart();
    }
  }

  function onPointerMove(e: React.PointerEvent) {
    const d = drag.current;
    if (!d) return;
    const dx = e.clientX - d.startX, dy = e.clientY - d.startY;
    if (!d.moved && Math.hypot(dx, dy) < CLICK_SLOP) return;
    d.moved = true;
    // Notes stay in their layers, so dragging one pans instead
    if (d.kind === 'pan' || !placed.simulation) {
      setView({ ...d.view, x: d.view.x + dx, y: d.view.y + dy });
    } else if (placed.simulation && d.id !== undefined) {
      const node = placed.at.get(d.id) as ForceNode;
      const rect = svg.current!.getBoundingClientRect();
      node.fx = (e.clientX - rect.left - view.x) / view.k;
      node.fy = (e.clientY - rect.top - view.y) / view.k;
    }
  }

  function onPointerUp() {
    const d = drag.current;
    drag.current = null;
    if (!d) return;
    if (d.kind === 'node' && placed.simulation) {
      const node = placed.at.get(d.id!) as ForceNode;
      node.fx = null;
      node.fy = null;
      placed.simulation.alphaTarget(0);
    }
    if (!d.moved) onSelect(d.kind === 'node' && d.id !== selected ? d.id! : null);
  }

  const byId = useMemo(() => new Map(nodes.map(n => [n.id, n])), [nodes]);
  const focus = hovered ?? selected;
  const family = useMemo(() => {
    if (focus === null) return null;
    const { ancestors, descendants } = lineage(focus, edges);
    return new Set([focus, ...ancestors, ...descendants]);
  }, [focus, edges]);
  const showAllLabels = view.k >= LABEL_ZOOM;
  const inFamily = (id: number) => family === null || family.has(id);

  return (
    <div style={{ position: 'relative', height }}>
      <svg
        ref={svg}
        width="100%"
        height="100%"
        role="img"
        aria-label={`Graph of ${nodes.length} notes and ${edges.length} links`}
        style={{ display: 'block', touchAction: 'none', cursor: 'grab', border: '1px solid var(--border-subtle)', borderRadius: 4, background: 'var(--surface)' }}
        onPointerDown={e => onPointerDown(e)}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
      >
        <defs>
          <marker id="graph-arrow" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
            <path d="M 0 0 L 10 5 L 0 10 z" fill="context-stroke" />
          </marker>
        </defs>
        <g transform={`translate(${view.x},${view.y}) scale(${view.k})`}>
          {edges.map(edge => {
            const from = placed.at.get(edge.to), to = placed.at.get(edge.from), dependent = byId.get(edge.from);
            if (!from || !to || !dependent) return null;
            // Stops at the edge of the note the arrow points to
            const length = Math.hypot(to.x - from.x, to.y - from.y) || 1;
            const cut = (radius(dependent) + 2) / length;
            const lit = family !== null && family.has(edge.from) && family.has(edge.to);
            return (
              <line
                key={`${edge.from}-${edge.to}`}
                x1={from.x}
                y1={from.y}
                x2={to.x - (to.x - from.x) * cut}
                y2={to.y - (to.y - from.y) * cut}
                stroke={lit ? 'var(--accent)' : 'var(--border-strong)'}
                strokeWidth={lit ? 1.5 : 1}
                opacity={family === null || lit ? 1 : 0.15}
                markerEnd="url(#graph-arrow)"
              >
                <title>{`${formatNoteId(edge.from)} links to ${formatNoteId(edge.to)} by "${edge.keyword}"${edge.score === null ? '' : ` (score ${edge.score})`}`}</title>
              </line>
            );
          })}
          {nodes.map(node => {
            const at = placed.at.get(node.id);
            if (!at) return null;
            const r = radius(node);
            const color = nodeColor(node, colorBy);
            const isSelected = node.id === selected;
            const showLabel = isSelected || node.id === hovered || marked.has(node.id) || (showAllLabels ? inFamily(node.id) : family !== null && family.has(node.id));
            const { new: n, learning, review, suspended } = node.cards;
            return (
              <g
                key={node.id}
                transform={`translate(${at.x},${at.y})`}
                opacity={inFamily(node.id) ? 1 : 0.15}
                style={{ cursor: 'pointer' }}
                onPointerDown={e => onPointerDown(e, node.id)}
                onPointerEnter={() => setHovered(node.id)}
                onPointerLeave={() => setHovered(h => (h === node.id ? null : h))}
              >
                <title>
                  {[
                    `${node.keywords.join(', ') || '(no keywords)'} · ${formatNoteId(node.id)}`,
                    node.in_query ? 'Found by the query' : `Built on by a note found, ${node.depth} link${node.depth === 1 ? '' : 's'} down`,
                    node.tags.length > 0 ? `Tags: ${node.tags.join(', ')}` : null,
                    `Cards: ${n} new, ${learning} learning, ${review} review${suspended ? `, ${suspended} suspended` : ''}`,
                  ].filter(Boolean).join('\n')}
                </title>
                {marked.has(node.id) && <circle r={r + 4} fill="none" stroke="var(--warning)" strokeWidth={2} />}
                <circle
                  r={r}
                  fill={color}
                  // Notes the query didn't find are hollow, whatever they are colored by
                  fillOpacity={node.in_query ? 1 : 0.25}
                  stroke={isSelected ? 'var(--text)' : color}
                  strokeWidth={isSelected ? 2.5 : 1.5}
                />
                {showLabel && (
                  <text
                    y={r + 4 + 10 / view.k}
                    textAnchor="middle"
                    fontSize={11 / view.k}
                    fill="var(--text-secondary)"
                    style={{ pointerEvents: 'none', userSelect: 'none' }}
                  >
                    {nodeLabel(node, formatNoteId)}
                  </text>
                )}
              </g>
            );
          })}
        </g>
      </svg>
      <div style={{ position: 'absolute', top: 8, right: 8, display: 'flex', gap: 4 }}>
        <button onClick={() => zoomBy(1.4)} className="touch-target-small" aria-label="Zoom in">+</button>
        <button onClick={() => zoomBy(1 / 1.4)} className="touch-target-small" aria-label="Zoom out">−</button>
        <button onClick={fit} className="touch-target-small" title="Fit every note in view">Fit</button>
      </div>
    </div>
  );
}
