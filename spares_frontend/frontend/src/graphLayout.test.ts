import { expect, test } from 'bun:test';
import { backEdges, bounds, forceLayout, formatGroup, layOut, layeredLayout, layers, lineage, maxRowLength, parseGroup, studyState, unmatchedNodes } from './graphLayout';
import type { GraphEdge } from './types/spares';

const edge = (from: number, to: number): GraphEdge => ({ from, to, keyword: '', score: 0 });

test('layers put each note below those it links to', () => {
  // 4 links to 2 and 3, which link to 1; 5 links to 1 and 4
  const edges = [edge(4, 2), edge(4, 3), edge(2, 1), edge(3, 1), edge(5, 1), edge(5, 4)];
  expect(Object.fromEntries(layers([1, 2, 3, 4, 5], edges))).toEqual({ 1: 0, 2: 1, 3: 1, 4: 2, 5: 3 });
});

test('a cycle is broken at one link', () => {
  const edges = [edge(1, 2), edge(2, 3), edge(3, 1)];
  expect(backEdges([1, 2, 3], edges).size).toBe(1);
  const layer = layers([1, 2, 3], edges);
  expect(new Set(layer.values())).toEqual(new Set([0, 1, 2]));
  // A note linking to itself
  expect(Object.fromEntries(layers([7], [edge(7, 7)]))).toEqual({ 7: 0 });
});

test('a long chain of links does not overflow the stack', () => {
  const ids = Array.from({ length: 20_000 }, (_, i) => i);
  const edges = ids.slice(1).map(i => edge(i, i - 1));
  expect(layers(ids, edges).get(19_999)).toBe(19_999);
});

test('the layered layout orders a layer to keep linked notes near each other', () => {
  // 3 links to 2 and 4 to 1, so the order of 3 and 4 follows that of 1 and 2
  const at = layeredLayout([1, 2, 3, 4], [edge(3, 2), edge(4, 1)]);
  expect(at.get(1)!.y).toBe(at.get(2)!.y);
  expect(at.get(3)!.y).toBeGreaterThan(at.get(1)!.y);
  expect(Math.sign(at.get(4)!.x - at.get(3)!.x)).toBe(Math.sign(at.get(1)!.x - at.get(2)!.x));
});

test('a wide layer is split into rows, still below what its notes build on', () => {
  // 1000 notes linking to none, and a chain of 3 under one of them
  const ids = Array.from({ length: 1003 }, (_, i) => i);
  const edges = [edge(1000, 0), edge(1001, 1000), edge(1002, 1001)];
  const at = layeredLayout(ids, edges);
  const xs = [...at.values()].map(p => p.x);
  const width = Math.max(...xs) - Math.min(...xs);
  expect(width).toBeLessThanOrEqual(maxRowLength(1003) * 60);
  // No two notes in one place
  expect(new Set([...at.values()].map(p => `${p.x},${p.y}`)).size).toBe(1003);
  const top = Math.max(...ids.slice(0, 1000).map(id => at.get(id)!.y));
  expect(at.get(1000)!.y).toBeGreaterThan(top);
  expect(at.get(1001)!.y).toBeGreaterThan(at.get(1000)!.y);
  expect(at.get(1002)!.y).toBeGreaterThan(at.get(1001)!.y);
});

test('the force layout places every note', () => {
  const nodes = [1, 2, 3].map(id => ({ id, radius: 5 }));
  forceLayout(nodes, [edge(2, 1), edge(3, 1), edge(3, 99)]);
  for (const node of nodes) expect(Number.isFinite(node.x) && Number.isFinite(node.y)).toBe(true);
});

test('layOut places every note, in either layout', () => {
  const nodes = [1, 2, 3].map(id => ({ id, radius: 5 }));
  for (const layout of ['force', 'layered'] as const) {
    const placed = layOut({ layout, nodes, edges: [edge(2, 1), edge(3, 2)] });
    expect(placed.map(([id]) => id).sort()).toEqual([1, 2, 3]);
    for (const [, x, y] of placed) expect(Number.isFinite(x) && Number.isFinite(y)).toBe(true);
  }
});

test('lineage follows links both ways, through cycles', () => {
  const edges = [edge(2, 1), edge(3, 2), edge(4, 3), edge(1, 3)];
  const { ancestors, descendants } = lineage(2, edges);
  expect([...ancestors].sort()).toEqual([1, 3]);
  expect([...descendants].sort()).toEqual([1, 3, 4]);
});

test('studyState', () => {
  expect(studyState({ new: 0, learning: 0, review: 0, suspended: 0 })).toBe('none');
  expect(studyState({ new: 0, learning: 0, review: 0, suspended: 2 })).toBe('suspended');
  expect(studyState({ new: 2, learning: 0, review: 0, suspended: 1 })).toBe('new');
  expect(studyState({ new: 1, learning: 0, review: 1, suspended: 0 })).toBe('started');
  expect(studyState({ new: 0, learning: 1, review: 0, suspended: 0 })).toBe('started');
  expect(studyState({ new: 0, learning: 0, review: 3, suspended: 1 })).toBe('learned');
});

test('bounds', () => {
  expect(bounds([{ x: 0, y: 0 }, { x: 10, y: 20 }], 5)).toEqual({ x: -5, y: -5, width: 20, height: 30 });
  expect(bounds([], 5)).toEqual({ x: -5, y: -5, width: 10, height: 10 });
});

test('groups round-trip through the URL, and take blue without a color', () => {
  expect(parseGroup(formatGroup({ color: 'red', query: 'tag=a:b' }))).toEqual({ color: 'red', query: 'tag=a:b' });
  expect(parseGroup('tag=exam')).toEqual({ color: 'blue', query: 'tag=exam' });
  expect(parseGroup('nope:x')).toEqual({ color: 'blue', query: 'nope:x' });
});

test('unmatched links make one node per keyword, ignoring case', () => {
  const { nodes, edges } = unmatchedNodes([
    { from: 1, keyword: 'Zeta' },
    { from: 2, keyword: 'zeta' },
    { from: 2, keyword: 'ZETA' },
    { from: 1, keyword: 'eta' },
  ]);
  expect(nodes.map(n => [n.id, n.keywords[0], n.relation])).toEqual([[-1, 'Zeta', 'unmatched'], [-2, 'eta', 'unmatched']]);
  expect(edges.map(e => [e.from, e.to])).toEqual([[1, -1], [2, -1], [1, -2]]);
});
