import { forceCollide, forceLink, forceManyBody, forceSimulation, forceX, forceY, type Simulation, type SimulationLinkDatum, type SimulationNodeDatum } from 'd3-force';
import type { GraphEdge, GraphNode, GraphNodeCards, GraphRelation, UnmatchedLink } from './types/spares';

export interface Point { x: number; y: number }

/** Space between notes in a layer of the layered layout. */
const LAYER_GAP_X = 60;
/** Space between layers. */
const LAYER_GAP_Y = 90;
/** Up and down passes ordering each layer by its neighbors. */
const ORDER_SWEEPS = 8;
/** Space between the rows a wide layer is split into. */
const WRAPPED_ROW_GAP = 36;

/**
 * The most notes a row of the layered layout has before its layer is split into several rows,
 * so a graph of many notes is about as wide as a screen, not a strip: about √(2n).
 */
export function maxRowLength(notes: number): number {
  return Math.max(20, Math.ceil(Math.sqrt(2 * notes)));
}

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

  // A wide layer is split into rows column by column, so its notes keep their order left to right
  const maxRow = maxRowLength(ids.length);
  const at = new Map<number, Point>();
  let top = 0;
  for (const row of rows) {
    if (!row) continue;
    const depth = Math.ceil(row.length / maxRow);
    const columns = Math.ceil(row.length / depth);
    row.forEach((id, i) => at.set(id, {
      x: (Math.floor(i / depth) - (columns - 1) / 2) * LAYER_GAP_X,
      y: top + (i % depth) * WRAPPED_ROW_GAP,
    }));
    top += (depth - 1) * WRAPPED_ROW_GAP + LAYER_GAP_Y;
  }
  return at;
}

export interface ForceNode extends SimulationNodeDatum { id: number; radius: number }

export type ForceSimulation = Simulation<ForceNode, SimulationLinkDatum<ForceNode>>;

/** The forces of the force layout, as Obsidian's graph view lets them be set. */
export interface GraphForces {
  /** How strongly notes are pulled to the middle, which makes the graph rounder. */
  center: number;
  /** How strongly notes push each other apart. */
  repel: number;
  /** How strongly links pull, as a multiple of d3's default, which is weaker for busy notes. */
  link: number;
  /** How long links try to be. */
  distance: number;
}

export const DEFAULT_FORCES: GraphForces = { center: 0.04, repel: 120, link: 1, distance: 50 };

/**
 * A simulation in which linked notes pull together and all notes push apart, as in Obsidian's graph
 * view, stopped. Notes with a position keep it.
 */
export function forceSimulationOf(nodes: ForceNode[], edges: GraphEdge[], forces = DEFAULT_FORCES): ForceSimulation {
  const ids = new Set(nodes.map(n => n.id));
  const links = edges.filter(e => ids.has(e.from) && ids.has(e.to)).map(e => ({ source: e.from, target: e.to }));
  const link = forceLink<ForceNode, SimulationLinkDatum<ForceNode>>(links).id(n => n.id).distance(forces.distance);
  // d3's default strength, scaled
  const defaultStrength = link.strength();
  link.strength((l, i, all) => forces.link * (defaultStrength as (l: SimulationLinkDatum<ForceNode>, i: number, all: SimulationLinkDatum<ForceNode>[]) => number)(l, i, all));
  return forceSimulation(nodes)
    .force('link', link)
    .force('charge', forceManyBody().strength(-forces.repel).distanceMax(600))
    // Pulls toward the middle, so notes with no links stay near the rest
    .force('x', forceX(0).strength(forces.center))
    .force('y', forceY(0).strength(forces.center))
    .force('collide', forceCollide<ForceNode>(n => n.radius + 2))
    .stop();
}

/**
 * Places `nodes` by running `forceSimulationOf` until it has nearly settled. Notes that already
 * have positions start from them, with less energy, so new forces move them rather than start
 * the layout over.
 */
export function forceLayout(nodes: ForceNode[], edges: GraphEdge[], ticks = 300, forces = DEFAULT_FORCES): ForceSimulation {
  const placed = nodes.length > 0 && nodes.every(n => n.x !== undefined);
  return forceSimulationOf(nodes, edges, forces).alpha(placed ? 0.5 : 1).tick(placed ? ticks / 2 : ticks);
}

/** What `graphLayout.worker.ts` is asked to lay out. */
export interface LayoutRequest {
  layout: 'force' | 'layered';
  /** With where a note is now, for the force layout to start from. */
  nodes: { id: number; radius: number; x?: number; y?: number }[];
  edges: GraphEdge[];
  forces?: GraphForces;
}

/** Each note's id and position, as the worker answers. */
export type LayoutResponse = [number, number, number][];

/** Lays out a graph, as the worker does off the page's thread. */
export function layOut({ layout, nodes, edges, forces }: LayoutRequest): LayoutResponse {
  if (layout === 'layered') {
    return [...layeredLayout(nodes.map(n => n.id), edges)].map(([id, { x, y }]) => [id, x, y]);
  }
  const forceNodes: ForceNode[] = nodes.map(n => ({ ...n }));
  forceLayout(forceNodes, edges, 300, forces);
  return forceNodes.map(n => [n.id, n.x!, n.y!]);
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

/** The colors a group of the graph can have, by name. */
export const GROUP_COLORS: Record<string, string> = {
  blue: 'var(--tone-blue-fg)',
  green: 'var(--tone-green-fg)',
  orange: 'var(--tone-orange-fg)',
  purple: 'var(--tone-purple-fg)',
  teal: 'var(--tone-teal-fg)',
  red: 'var(--tone-red-fg)',
  pink: 'var(--chart-relearning)',
};

/** A group of the graph: the notes `query` finds, drawn in `color`, a key of `GROUP_COLORS`. */
export interface GraphGroup { color: string; query: string }

/** A group as kept in the URL, as in `red:tag=exam`. */
export function parseGroup(param: string): GraphGroup {
  const at = param.indexOf(':');
  const color = at < 0 ? '' : param.slice(0, at);
  return color in GROUP_COLORS ? { color, query: param.slice(at + 1) } : { color: 'blue', query: param };
}

export const formatGroup = ({ color, query }: GraphGroup) => `${color}:${query}`;

/**
 * Nodes for the keywords `unmatched` links search for, which no note has, one per keyword
 * ignoring case, with ids below 0, and an edge to each from each note with such a link.
 */
export function unmatchedNodes(unmatched: UnmatchedLink[]): { nodes: GraphNode[]; edges: GraphEdge[] } {
  const ids = new Map<string, number>();
  const nodes: GraphNode[] = [];
  const edges: GraphEdge[] = [];
  const seen = new Set<string>();
  for (const { from, keyword } of unmatched) {
    const key = keyword.toLowerCase();
    let id = ids.get(key);
    if (id === undefined) {
      id = -(ids.size + 1);
      ids.set(key, id);
      nodes.push({ id, keywords: [keyword], tags: [], relation: 'unmatched', depth: 0, group: null, cards: { new: 0, learning: 0, review: 0, suspended: 0 } });
    }
    if (!seen.has(`${from} ${id}`)) {
      seen.add(`${from} ${id}`);
      edges.push({ from, to: id, keyword, score: null });
    }
  }
  return { nodes, edges };
}

/** The colors of `relation`, when notes are colored by it. */
export const RELATION_COLORS: Record<GraphRelation, { color: string; label: string }> = {
  found: { color: 'var(--accent)', label: 'Found' },
  prerequisite: { color: 'var(--text-faint)', label: 'Built on' },
  dependent: { color: 'var(--tone-purple-fg)', label: 'Building on them' },
  unmatched: { color: 'var(--text-muted)', label: 'No such note' },
};

/** How the graph is drawn, as Obsidian's graph view lets it be set. */
export interface GraphDisplay {
  /** What a note's size shows: how many cards it has, or how many links it has either way. */
  sizeBy: 'cards' | 'links';
  /** A multiple of the size. */
  nodeSize: number;
  /** The width of links. */
  linkWidth: number;
  /** The zoom at which every note is named. Names fade in as the graph is zoomed in to it. */
  labelZoom: number;
  arrows: boolean;
}

export const DEFAULT_DISPLAY: GraphDisplay = { sizeBy: 'cards', nodeSize: 1, linkWidth: 1, labelZoom: 0.7, arrows: true };

/** A note's radius, given how many links it has either way. */
export function nodeRadius(node: GraphNode, links: number, display: GraphDisplay): number {
  const { new: n, learning, review, suspended } = node.cards;
  const base = node.relation === 'unmatched'
    ? 4
    : display.sizeBy === 'links'
      ? Math.min(16, 4 + 2 * Math.sqrt(links))
      : Math.min(16, 4 + 1.5 * Math.sqrt(n + learning + review + suspended));
  return base * display.nodeSize;
}

/** How visible notes' names are at zoom `k`: fading in from 60% of `labelZoom` to all of it. */
export function labelOpacity(k: number, labelZoom: number): number {
  return Math.min(1, Math.max(0, (k - 0.6 * labelZoom) / (0.4 * labelZoom)));
}
