import { forceCollide, forceLink, forceManyBody, forceSimulation, forceX, forceY, type Simulation, type SimulationLinkDatum, type SimulationNodeDatum } from 'd3-force';
import type { GraphEdge, GraphNode, GraphNodeCards } from './types/spares';

export interface Point { x: number; y: number }

/** Space between notes in a layer of the layered layout. */
const LAYER_GAP_X = 60;
/** Space between layers. */
const LAYER_GAP_Y = 90;
/** Up and down passes ordering each layer by its neighbors. */
const ORDER_SWEEPS = 8;

/**
 * The links that close cycles, found depth first, so that leaving them out leaves links that only
 * go up, from a note to one it builds on. Links normally do: a note links to older notes.
 */
export function backEdges(ids: number[], edges: GraphEdge[]): Set<GraphEdge> {
  const out = new Map<number, GraphEdge[]>(ids.map(id => [id, []]));
  for (const edge of edges) out.get(edge.from)?.push(edge);
  const state = new Map<number, 'open' | 'done'>();
  const back = new Set<GraphEdge>();
  for (const root of ids) {
    if (state.has(root)) continue;
    // Iterative, as chains of links can be deeper than the stack allows
    const stack: [number, number][] = [[root, 0]];
    state.set(root, 'open');
    while (stack.length > 0) {
      const top = stack[stack.length - 1];
      const [id, next] = top;
      const edge = out.get(id)?.[next];
      if (!edge) {
        state.set(id, 'done');
        stack.pop();
        continue;
      }
      top[1]++;
      const seen = state.get(edge.to);
      if (seen === 'open') back.add(edge);
      else if (seen === undefined && out.has(edge.to)) {
        state.set(edge.to, 'open');
        stack.push([edge.to, 0]);
      }
    }
  }
  return back;
}

/**
 * Each note's layer: 0 for notes that link to none, and otherwise one more than the highest layer
 * of the notes they link to, so every note is below everything it builds on.
 */
export function layers(ids: number[], edges: GraphEdge[]): Map<number, number> {
  const back = backEdges(ids, edges);
  const prerequisites = new Map<number, number[]>(ids.map(id => [id, []]));
  for (const edge of edges) {
    if (!back.has(edge) && prerequisites.has(edge.to)) prerequisites.get(edge.from)?.push(edge.to);
  }
  const layer = new Map<number, number>();
  for (const root of ids) {
    if (layer.has(root)) continue;
    // Depth first, placing a note once those it links to are placed
    const stack = [root];
    while (stack.length > 0) {
      const id = stack[stack.length - 1];
      const pending = prerequisites.get(id)!.filter(p => !layer.has(p));
      if (pending.length > 0) { stack.push(...pending); continue; }
      stack.pop();
      if (!layer.has(id)) layer.set(id, Math.max(-1, ...prerequisites.get(id)!.map(p => layer.get(p)!)) + 1);
    }
  }
  return layer;
}

/**
 * Notes in layers, with the notes they build on above them, each layer ordered to keep linked
 * notes near each other.
 */
export function layeredLayout(ids: number[], edges: GraphEdge[]): Map<number, Point> {
  const layer = layers(ids, edges);
  const rows: number[][] = [];
  for (const id of ids) (rows[layer.get(id)!] ??= []).push(id);
  const up = new Map<number, number[]>(ids.map(id => [id, []]));
  const down = new Map<number, number[]>(ids.map(id => [id, []]));
  for (const edge of edges) {
    if (!layer.has(edge.from) || !layer.has(edge.to)) continue;
    up.get(edge.from)!.push(edge.to);
    down.get(edge.to)!.push(edge.from);
  }

  const position = new Map<number, number>();
  const place = (row: number[]) => row.forEach((id, i) => position.set(id, i - (row.length - 1) / 2));
  rows.forEach(place);
  // Each note goes to the mean position of its neighbors in the layers already ordered
  const sweep = (row: number[], neighbors: Map<number, number[]>) => {
    const keyed = row.map(id => {
      const ps = neighbors.get(id)!.map(n => position.get(n)!);
      return { id, key: ps.length > 0 ? ps.reduce((a, b) => a + b, 0) / ps.length : position.get(id)! };
    });
    keyed.sort((a, b) => a.key - b.key);
    row.splice(0, row.length, ...keyed.map(k => k.id));
    place(row);
  };
  for (let i = 0; i < ORDER_SWEEPS; i++) {
    if (i % 2 === 0) for (const row of rows.slice(1)) sweep(row, up);
    else for (const row of rows.slice(0, -1).reverse()) sweep(row, down);
  }

  return new Map(ids.map(id => [id, { x: position.get(id)! * LAYER_GAP_X, y: layer.get(id)! * LAYER_GAP_Y }]));
}

export interface ForceNode extends SimulationNodeDatum { id: number; radius: number }

/**
 * A simulation in which linked notes pull together and all notes push apart, as in Obsidian's graph
 * view, run until it has nearly settled. It is stopped, to be ticked again when a note is dragged.
 */
export function forceLayout(nodes: ForceNode[], edges: GraphEdge[], ticks = 300): Simulation<ForceNode, SimulationLinkDatum<ForceNode>> {
  const ids = new Set(nodes.map(n => n.id));
  const links = edges.filter(e => ids.has(e.from) && ids.has(e.to)).map(e => ({ source: e.from, target: e.to }));
  const simulation = forceSimulation(nodes)
    .force('link', forceLink<ForceNode, SimulationLinkDatum<ForceNode>>(links).id(n => n.id).distance(50))
    .force('charge', forceManyBody().strength(-120).distanceMax(600))
    // Pulls toward the middle, so notes with no links stay near the rest
    .force('x', forceX(0).strength(0.04))
    .force('y', forceY(0).strength(0.04))
    .force('collide', forceCollide<ForceNode>(n => n.radius + 2))
    .stop();
  simulation.tick(ticks);
  return simulation;
}

/** The notes reached from `start` by following `next`, leaving out `start`. */
function reach(start: number, next: Map<number, number[]>): Set<number> {
  const seen = new Set<number>();
  const queue = [start];
  while (queue.length > 0) {
    for (const n of next.get(queue.pop()!) ?? []) {
      if (n !== start && !seen.has(n)) { seen.add(n); queue.push(n); }
    }
  }
  return seen;
}

/** The notes `id` builds on, directly or not, and those that build on it. */
export function lineage(id: number, edges: GraphEdge[]): { ancestors: Set<number>; descendants: Set<number> } {
  const up = new Map<number, number[]>();
  const down = new Map<number, number[]>();
  for (const { from, to } of edges) {
    (up.get(from) ?? up.set(from, []).get(from)!).push(to);
    (down.get(to) ?? down.set(to, []).get(to)!).push(from);
  }
  return { ancestors: reach(id, up), descendants: reach(id, down) };
}

export type StudyState = 'none' | 'suspended' | 'new' | 'started' | 'learned';

/**
 * How far along a note's cards are: `new` if none has been studied, `learned` if all are in review,
 * and `started` in between. Suspended cards don't count unless all are.
 */
export function studyState(cards: GraphNodeCards): StudyState {
  const active = cards.new + cards.learning + cards.review;
  if (active === 0) return cards.suspended > 0 ? 'suspended' : 'none';
  if (cards.new === active) return 'new';
  if (cards.review === active) return 'learned';
  return 'started';
}

/** A note's name in the graph: its first keyword, or its id. */
export function nodeLabel(node: GraphNode, formatId: (id: number) => string): string {
  return node.keywords[0] ?? formatId(node.id);
}

/** The bounds of `points`, padded. */
export function bounds(points: Iterable<Point>, pad = 40): { x: number; y: number; width: number; height: number } {
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (const { x, y } of points) {
    minX = Math.min(minX, x); minY = Math.min(minY, y);
    maxX = Math.max(maxX, x); maxY = Math.max(maxY, y);
  }
  if (minX === Infinity) return { x: -pad, y: -pad, width: 2 * pad, height: 2 * pad };
  return { x: minX - pad, y: minY - pad, width: maxX - minX + 2 * pad, height: maxY - minY + 2 * pad };
}

const TAG_COLORS = ['--tone-blue-fg', '--tone-green-fg', '--tone-orange-fg', '--tone-purple-fg', '--tone-teal-fg', '--tone-red-fg', '--chart-relearning'];

export const STUDY_COLORS: Record<StudyState, { color: string; label: string }> = {
  new: { color: 'var(--tone-red-fg)', label: 'Not studied' },
  started: { color: 'var(--chart-learning)', label: 'Partly learned' },
  learned: { color: 'var(--chart-mature)', label: 'Learned' },
  suspended: { color: 'var(--text-faint)', label: 'Suspended' },
  none: { color: 'var(--border-strong)', label: 'No cards' },
};

/** A tag's color, by its top-level name, so `math:algebra` and `math:calculus` match. */
export function tagColor(tag: string | undefined): string {
  if (tag === undefined) return 'var(--text-faint)';
  const top = tag.split(':')[0].toLowerCase();
  let hash = 0;
  for (const c of top) hash = (hash * 31 + c.charCodeAt(0)) | 0;
  return `var(${TAG_COLORS[Math.abs(hash) % TAG_COLORS.length]})`;
}
