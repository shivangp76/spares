// The demo collection's notes (see build.ts). Written for the demo, on general topics, so that it
// shows each feature the README lists, in each parser where it applies:
//
// - clozes and their settings: hints (`h:`), groupings (`g:`, `g:*`), reverse and reverse-only
//   cards (`r`, `ro`, `rtag:`), suspended clozes (`s`), clozes hidden without being asked
//   (`hide:`), all clozes hidden on the front and only the answer shown on the back (`f:all`,
//   `b:a`), emphasis on the back (`be:`), typed answers (`t`), overlapping lists (`ov`), nested
//   clozes, text to speech (`tts:`), card tags (`tag:`), and a card's history copied (`inh:`) or
//   moved (`mv:`) from another note's card (LATER_NOTES);
// - links between notes, with fuzzy matches (plurals), links across topics, Typst's dependent and
//   related links, a link to a keyword no note has, a loop of "see also" links, and a
//   prerequisite created after the note that needs it;
// - keywords, including sources and keywords shared by several notes, and a tag hierarchy three
//   levels deep;
// - two image occlusions (OCCLUSIONS), the second with hints, markup and text clozes in the same
//   cards, an image and audio clips in Markdown notes, and the markdown-language-tools parser's
//   readings (dictionary lookup needs a host, which the demo hasn't);
// - a review history with leeches, a card forgotten and relearned, a card buried on two days,
//   crams in a filtered tag that doesn't reschedule, and days off.
//
// Each note's `ago` is how many days before the build it was created, which its review history
// starts from.

export type ParserName = "markdown" | "typst" | "latex-note" | "markdown-language-tools";

export interface DemoNote {
  parser: ParserName;
  /**
   * The note's text. `@image(name)` and `@audio(name)` stand for the snippet of the file `name` in
   * this directory, uploaded as an image or an audio clip, and `@card(keyword/order)` for the card
   * of that order of the note whose first keyword is `keyword` (`NOTE_ID/ORDER`, for `inh:` and
   * `mv:`).
   */
  data: string;
  tags: string[];
  keywords?: string[];
  /** Days before the build it was created. Notes created today have no reviews. */
  ago: number;
  /**
   * How well its cards are known, from 0 (often forgotten) to 1 (never), for the made-up reviews.
   * Below `LEECH_EASE`, cards lapse often enough to become leeches.
   */
  ease?: number;
  suspended?: boolean;
  /** The note's custom data, such as `{"language": "ja"}` for markdown-language-tools */
  customData?: Record<string, unknown>;
  /** Days before the build its cards were forgotten (reset to new) and started again */
  forgotten?: number;
  /** Its first card is buried today, and was on another day without being rated in between */
  buried?: boolean;
}

/** An `ease` below this makes leeches (see build.ts) */
export const LEECH_EASE = 0.2;

const rust = (data: string, keywords: string[], ago: number, ease = 0.8): DemoNote => ({
  parser: "markdown",
  data,
  tags: ["programming:rust"],
  keywords,
  ago,
  ease,
});

const algorithms = (data: string, keywords: string[], ago: number, ease = 0.75): DemoNote => ({
  parser: "markdown",
  data,
  tags: ["cs:algorithms"],
  keywords,
  ago,
  ease,
});

const linearAlgebra = (data: string, keywords: string[], ago: number, ease = 0.7): DemoNote => ({
  parser: "typst",
  data,
  tags: ["math:linear-algebra"],
  keywords,
  ago,
  ease,
});

const calculus = (
  parser: ParserName,
  data: string,
  keywords: string[],
  ago: number,
  ease = 0.75,
): DemoNote => ({ parser, data, tags: ["math:calculus"], keywords, ago, ease });

/** A note with readings (`漢字[かんじ]`), in `language` */
const withReadings =
  (tag: string, language: string, tts: string) =>
  (data: string, keywords: string[], ago: number, ease = 0.75, customData: Record<string, unknown> = {}): DemoNote => ({
    parser: "markdown-language-tools",
    data,
    tags: [tag],
    keywords,
    ago,
    ease,
    customData: { language, tts, ...customData },
  });
const japanese = withReadings("languages:japanese", "ja", "ja_JP");
const chinese = withReadings("languages:chinese", "zh", "zh_CN");

const spanish = (data: string, keywords: string[], ago: number, ease = 0.85): DemoNote => ({
  parser: "markdown",
  data,
  tags: ["languages:spanish"],
  keywords,
  ago,
  ease,
});

/** Notes of `parser` tagged `tag`, for topics with a parser of their own */
const topic =
  (parser: ParserName, tag: string, defaultEase = 0.75) =>
  (data: string, keywords: string[], ago: number, ease = defaultEase): DemoNote => ({
    parser,
    data,
    tags: [tag],
    keywords,
    ago,
    ease,
  });
const genetics = topic("markdown", "science:biology:genetics");
const cells = topic("markdown", "science:biology:cells");
const chemistry = topic("typst", "science:chemistry", 0.7);
const physics = topic("latex-note", "science:physics", 0.7);
const astronomy = topic("markdown", "science:physics:astronomy", 0.8);
const history = topic("markdown", "history", 0.8);
const geography = topic("markdown", "geography", 0.85);
const music = topic("markdown", "music:theory", 0.8);

export const NOTES: DemoNote[] = [
  // Rust
  rust(
    `# Ownership

- Each value in Rust has exactly {{one owner}}.
- When the owner goes out of scope, the value is {{dropped}}.
- Assigning a \`String\` to another variable {{moves}} it: the first variable can no longer be used.

\`\`\`rust
let a = String::from("hi");
let b = a;
// println!("{a}"); // error: borrow of {{[h:what happened to a?] moved value}}
\`\`\``,
    ["Ownership", "Move semantics"],
    88,
  ),
  rust(
    `# Borrowing

At any time, you can have either {{[g:1] one mutable reference}} or {{[g:1] any number of immutable references}} to a value, but not both.

References must always be {{valid}}: the compiler rejects a reference that outlives its value, built on [ownership][li].`,
    ["Borrowing", "References"],
    85,
  ),
  rust(
    `# \`Option<T>\`

Rust has no null. A value that may be absent is an \`Option<T>\`, which is either {{\`Some(T)\`}} or {{\`None\`}}.

\`opt.unwrap_or(default)\` returns the value inside, or {{\`default\` if it is \`None\`}}.`,
    ["Option", "Option<T>"],
    80,
  ),
  rust(
    `# \`Result<T, E>\` and \`?\`

The \`?\` operator, applied to a \`Result\`, returns the {{value inside \`Ok\`}}, or returns early from the function with the {{\`Err\`}}, converted with {{[tag:hard] \`From::from\`}}.

Compare [Option<T>][li].`,
    ["Result", "Question mark operator"],
    76,
  ),
  rust(
    `# Traits

A trait defines {{shared behaviour}} that types can implement, like an interface.

\`\`\`rust
trait Area {
    fn area(&self) -> f64;
}
impl Area for Circle {
    fn area(&self) -> f64 { {{std::f64::consts::PI * self.r * self.r}} }
}
\`\`\`

\`fn print(x: &impl Area)\` is shorthand for a {{[h:syntax] generic with a trait bound}}: \`fn print<T: Area>(x: &T)\`.`,
    ["Traits"],
    70,
  ),
  // The card you keep putting off: buried today, and on another day without a rating between
  {
    ...rust(
      `# Lifetimes

\`fn longest<'a>(x: &'a str, y: &'a str) -> &'a str\` says that the returned reference lives {{[tag:hard] at least as long as the shorter of \`x\` and \`y\`}}.

Lifetimes are part of [borrowing][li]: they only describe how long references are valid, they never {{change how long values live}}.`,
      ["Lifetimes"],
      64,
      0.55,
    ),
    buried: true,
  },
  rust(
    `# Iterators

Iterator adapters such as \`map\` and \`filter\` are {{lazy}}: nothing happens until a consumer such as {{\`collect\`}} or \`sum\` runs them.

\`\`\`rust
let squares: Vec<i32> = (1..=3).map(|x| x * x).collect();
// squares == {{[t] [1, 4, 9]}}
\`\`\``,
    ["Iterators"],
    58,
  ),
  rust(
    `# Smart pointers

| Type | Use |
|---|---|
| \`Box<T>\` | {{one owner, value on the heap}} |
| \`Rc<T>\` | {{several owners, one thread}} |
| \`Arc<T>\` | {{several owners, shared across threads}} |
| \`RefCell<T>\` | {{[tag:hard] borrow rules checked at run time}} |`,
    ["Smart pointers", "Box", "Rc"],
    45,
    0.6,
  ),
  rust(
    `# Pattern matching

A \`match\` must be {{exhaustive}}: every possible value must be covered, which is why \`_\` exists as a {{catch-all}} pattern.

\`if let Some(x) = opt { … }\` matches {{one pattern}} and ignores the rest. See [Option<T>][li].`,
    ["Pattern matching", "match"],
    30,
  ),
  rust(
    `# \`Send\` and \`Sync\`

- A type is \`Send\` if it is safe to {{move to another thread}}.
- A type is \`Sync\` if it is safe to {{share a reference to it between threads}}: \`&T\` is \`Send\`.
- \`Rc<T>\` is {{neither}}, which is why threads use \`Arc<T>\` (see [smart pointers][li]).`,
    ["Send", "Sync", "Concurrency"],
    12,
    0.5,
  ),
  rust(
    `# Closures

A closure that moves captured values into itself is declared with the {{\`move\`}} keyword. The three closure traits, from most to least permissive to call, are {{[ov] \`Fn\`}}, {{[ov] \`FnMut\`}} and {{[ov] \`FnOnce\`}}.`,
    ["Closures"],
    3,
  ),
  // An image, uploaded with the note and shown on its cards
  rust(
    `# Stack and heap

@image(string.png)

A \`String\` is three words on the stack: a {{pointer}} to its bytes, its {{length}} and its {{capacity}}. The bytes themselves are on the {{heap}}, so moving the \`String\` copies {{[h:how many bytes?] only the three words}}, which is why [ownership][li] moves it rather than copying it.`,
    ["Stack and heap", "String"],
    61,
  ),
  // A leech: forgotten again and again
  rust(
    `# Interior mutability

Interior mutability lets code change a value through a {{shared reference}} (\`&T\`).

- \`Cell<T>\` moves values {{in and out}}, and never hands out a reference to what is inside.
- \`RefCell<T>\` checks the [borrowing][li] rules {{at run time}}, and {{panics}} when they are broken.
- Across threads, use {{\`Mutex<T>\`}} or \`RwLock<T>\` instead, which are [Sync][li].`,
    ["Interior mutability", "Cell", "RefCell"],
    84,
    0.1,
  ),
  rust(
    `# \`String\` and \`&str\`

| | Owns its bytes | Can grow |
|---|---|---|
| \`String\` | {{[g:1] yes}} | {{[g:1] yes}} |
| \`&str\` | {{[g:2] no: it borrows them}} | {{[g:2] no}} |

A \`&String\` turns into a \`&str\` by itself, through {{[t] Deref}}, so functions take {{[h:which type?] \`&str\`}} to accept both.`,
    ["String slice", "&str"],
    27,
  ),

  // Algorithms
  algorithms(
    `# Binary search

Binary search finds a value in a {{sorted}} array in {{O(log n)}} time, by halving the range that may hold it at every step.

\`\`\`python
def search(xs, target):
    lo, hi = 0, len(xs)
    while lo < hi:
        mid = (lo + hi) // 2
        if xs[mid] < target:
            lo = {{mid + 1}}
        else:
            hi = mid
    return lo
\`\`\``,
    ["Binary search"],
    89,
    0.85,
  ),
  algorithms(
    `# Big-O of common operations

| Structure | Lookup | Insert |
|---|---|---|
| Array (by index) | {{O(1)}} | O(n) |
| Hash table | {{O(1) on average}} | O(1) on average |
| Balanced BST | {{O(log n)}} | O(log n) |
| Linked list | O(n) | {{O(1) at the head}} |`,
    ["Big-O", "Complexity"],
    86,
  ),
  algorithms(
    `# Hash tables

A hash table maps a key to a bucket with a {{hash function}}. Two keys in the same bucket are a {{collision}}, resolved by {{[g:1] chaining}} or {{[g:1] open addressing}}.

As the {{load factor}} grows, the table is resized, which keeps inserts O(1) {{[tag:hard] amortized}}. See [Big-O][li].`,
    ["Hash table", "Hash map"],
    82,
  ),
  algorithms(
    `# Quicksort

Quicksort picks a {{pivot}}, {{partitions}} the array around it, and sorts both sides recursively.

- Average time: {{O(n log n)}}
- Worst case, e.g. a sorted array with the first element as pivot: {{[tag:hard] O(n^2)}}
- It sorts {{in place}}, but it is {{not stable}}.`,
    ["Quicksort", "Sorting"],
    78,
    0.6,
  ),
  algorithms(
    `# Merge sort

Merge sort splits the array in halves, sorts each, and {{merges}} them. It takes {{O(n log n)}} time in every case, and is {{stable}}, but needs {{O(n)}} extra memory. Compare [quicksort][li].`,
    ["Merge sort"],
    74,
  ),
  algorithms(
    `# BFS and DFS

- Breadth-first search uses a {{queue}}, and finds {{shortest paths in an unweighted graph}}.
- Depth-first search uses a {{stack}} (or recursion), and is used for {{[h:ordering] topological sort}} and finding cycles.
- Both take {{O(V + E)}} time.`,
    ["BFS", "DFS", "Graph search"],
    68,
  ),
  algorithms(
    `# Dijkstra's algorithm

Dijkstra's algorithm finds shortest paths from one source in a graph with {{non-negative}} edge weights. It repeatedly takes the closest unvisited vertex from a {{priority queue}}.

With a binary heap it takes {{[tag:hard] O((V + E) log V)}}. With negative weights, use {{Bellman–Ford}} instead (see [Bellman-Ford][li]). It is [BFS][li] with a [priority queue][li] in place of the queue.`,
    ["Dijkstra"],
    60,
    0.55,
  ),
  algorithms(
    `# Dynamic programming

A problem suits dynamic programming when it has {{[g:1] optimal substructure}} and {{[g:1] overlapping subproblems}}.

Solving it top-down, by caching a recursive function's results, is called {{memoization}}. Filling a table from the smallest subproblems up is {{tabulation}}.`,
    ["Dynamic programming", "Memoization"],
    50,
  ),
  algorithms(
    `# Heaps

A binary heap is a complete binary tree where each parent is {{smaller than its children}} (a min-heap). Stored in an array, the children of index \`i\` are at {{\`2i + 1\` and \`2i + 2\`}}.

- Find the minimum: {{O(1)}}
- Insert or remove the minimum: {{O(log n)}}
- Build a heap from n items: {{[tag:hard] O(n)}}`,
    ["Heap", "Priority queue"],
    40,
  ),
  algorithms(
    `# Two pointers

To find two numbers in a {{sorted}} array that sum to a target, start a pointer at each end: move the left one right if the sum is {{too small}}, and the right one left if it is {{too large}}. It takes {{O(n)}} time and O(1) memory.`,
    ["Two pointers"],
    22,
  ),
  algorithms(
    `# Union–find

Union–find keeps a partition of elements into sets, with \`find(x)\` returning a set's representative and \`union(x, y)\` merging two sets.

With {{[g:1] path compression}} and {{[g:1] union by rank}}, each operation takes {{[tag:hard] amortized O(alpha(n))}} time, where alpha is the inverse Ackermann function: nearly constant.`,
    ["Union-find", "Disjoint set"],
    8,
    0.5,
  ),
  algorithms(
    `# Tries

A trie stores strings by their characters, one edge per character, so all words with the same {{prefix}} share a path from the root. Looking up a word of length m takes {{O(m)}}, however many words are stored.`,
    ["Trie", "Prefix tree"],
    0,
  ),
  algorithms(
    `# Topological sort

A topological order of a {{directed acyclic graph}} lists every vertex before the vertices its edges point to.

- Kahn's algorithm repeatedly removes a vertex with {{in-degree 0}}.
- Or run [DFS][li], and list vertices in {{reverse order of finishing}}.

If some vertices are never removed, the graph has a {{cycle}}. Both take O(V + E).`,
    ["Topological sort", "Kahn's algorithm"],
    36,
  ),
  // With Dijkstra's note, a loop of links: each says "see the other" (`in_link_cycle`)
  algorithms(
    `# Bellman–Ford

Bellman–Ford finds shortest paths from one source even with {{negative}} edge weights, by relaxing every edge {{V - 1}} times, in {{O(V E)}} time.

If a further round still shortens a path, the graph has a {{[h:what kind?] negative cycle}}. Compare [Dijkstra][li], which is faster but needs non-negative weights.`,
    ["Bellman-Ford", "Negative cycle"],
    25,
    0.6,
  ),
  // Merged into one note today (LATER_NOTES), moving their cards and reviews with `mv:`
  algorithms(
    `# Stacks

A stack is {{[h:acronym] LIFO}}: the last item pushed is the first popped. Push and pop take {{O(1)}}.`,
    ["Stack", "LIFO"],
    71,
    0.85,
  ),
  algorithms(
    `# Queues

A queue is {{[h:acronym] FIFO}}: items leave in the order they arrived. A ring buffer gives enqueue and dequeue in {{O(1)}}.`,
    ["Queue", "FIFO"],
    70,
    0.85,
  ),

  // Linear algebra (Typst)
  linearAlgebra(
    `= Matrix multiplication

The product $A B$ of an $m times n$ and an $n times p$ matrix is #cl[an $m times p$ matrix], with entries
$ (A B)_(i j) = #cl[$sum_(k=1)^n a_(i k) b_(k j)$] $

Matrix multiplication is #cl[associative] but not #cl[commutative].`,
    ["Matrix multiplication"],
    87,
    0.8,
  ),
  linearAlgebra(
    `= Determinant

For a $2 times 2$ matrix,
$ det mat(a, b; c, d) = #cl[$a d - b c$] $

$det(A B) = #cl[$det(A) det(B)$]$, and $A$ is invertible exactly when #cl[$det(A) != 0$].

The determinant is the factor by which $A$ scales #cl[volumes][h:geometry].`,
    ["Determinant"],
    83,
  ),
  linearAlgebra(
    `= Inverse matrix

$A^(-1)$ exists exactly when #lin[the determinant] is #cl[non-zero]. For a $2 times 2$ matrix,
$ mat(a, b; c, d)^(-1) = #cl[$1 / (a d - b c) mat(d, -b; -c, a)$][tag:hard] $

$(A B)^(-1) = #cl[$B^(-1) A^(-1)$]$.`,
    ["Inverse matrix"],
    79,
    0.6,
  ),
  linearAlgebra(
    `= Eigenvalues and eigenvectors

A non-zero vector $v$ is an eigenvector of $A$ with eigenvalue $lambda$ when #cl[$A v = lambda v$].

The eigenvalues are the roots of the #cl[characteristic polynomial] $det(A - lambda I) = 0$, which uses #lin[the determinant].

The sum of the eigenvalues is the #cl[trace][g:1], and their product the #cl[determinant][g:1].`,
    ["Eigenvalue", "Eigenvector"],
    72,
  ),
  // Forgotten (reset to new) 25 days ago, and learned again since
  {
    ...linearAlgebra(
      `= Rank–nullity theorem

For a linear map $T: V -> W$ with $V$ finite-dimensional,
$ dim V = #cl[$"rank"(T) + "nullity"(T)$] $
where the rank is the dimension of the #cl[image] and the nullity that of the #cl[kernel].`,
      ["Rank-nullity theorem"],
      65,
      0.55,
    ),
    forgotten: 25,
  },
  linearAlgebra(
    `= Dot product

$u dot v = |u| |v| cos theta$, so $u dot v = 0$ exactly when $u$ and $v$ are #cl[orthogonal].

In coordinates, $u dot v = #cl[$sum_i u_i v_i$]$.

The projection of $u$ onto $v$ is #cl[$(u dot v) / (v dot v) v$][tag:hard].`,
    ["Dot product", "Inner product"],
    55,
  ),
  linearAlgebra(
    `= Orthogonal matrices

A square matrix $Q$ is orthogonal when #cl[$Q^T Q = I$], so its inverse is #cl[its transpose].

Orthogonal matrices preserve #cl[lengths and angles], and their determinant is #cl[$plus.minus 1$].

They are what the #lin(rel: "dependent")[spectral theorem] diagonalizes symmetric matrices with.`,
    ["Orthogonal matrix", "Orthogonal"],
    35,
  ),
  linearAlgebra(
    `= Spectral theorem

A real #cl[symmetric] matrix $A$ can be written $A = Q Lambda Q^T$, where $Q$ is #lin[orthogonal] and $Lambda$ is #cl[diagonal, holding the eigenvalues].

So its #lin[eigenvalues] are all #cl[real], and eigenvectors of distinct eigenvalues are #cl[orthogonal].`,
    ["Spectral theorem"],
    18,
    0.5,
  ),
  linearAlgebra(
    `= Singular value decomposition

Every $m times n$ matrix can be written $A = #cl[$U Sigma V^T$]$, with $U$ and $V$ #cl[orthogonal] and $Sigma$ diagonal with non-negative entries, the #cl[singular values].

The singular values are the square roots of the eigenvalues of #cl[$A^T A$][tag:hard]. Compare the #lin(rel: "related")[spectral theorem].`,
    ["SVD", "Singular value decomposition"],
    5,
    0.45,
  ),
  linearAlgebra(
    `= Linear independence

Vectors $v_1, dots, v_k$ are linearly independent when $c_1 v_1 + dots.c + c_k v_k = 0$ only for #cl[$c_1 = dots.c = c_k = 0$].

In $RR^n$, more than #cl[$n$] vectors are always dependent. The columns of a square matrix are independent exactly when #lin[the determinant] is #cl[non-zero].

The word for a set of independent vectors that spans the space: #cl[basis][t].`,
    ["Linear independence", "Basis"],
    80,
    0.8,
  ),
  linearAlgebra(
    `= Gram–Schmidt process

Gram–Schmidt turns a #lin[basis] into an #lin[orthogonal] one: from each vector, subtract its #cl[projections onto the vectors already made], using #lin[the dot product].

Normalizing them too gives an #cl[orthonormal] basis, and writing $A$'s columns in it gives the #cl[$Q R$] decomposition.

It is what the #lin(rel: "dependent")[least squares] method uses to solve its equations stably.`,
    ["Gram-Schmidt process", "QR decomposition"],
    31,
    0.6,
  ),
  linearAlgebra(
    `= Least squares

When $A x = b$ has no solution, the least squares solution $hat(x)$ minimizes #cl[$|A x - b|$]. It solves the #cl[normal equations]
$ #cl[$A^T A hat(x) = A^T b$][tag:hard] $
so $A hat(x)$ is the #cl[projection of $b$ onto the column space of $A$], and $b - A hat(x)$ is #lin[orthogonal] to it.

Fitting a line $y = m x + c$ to points is least squares with $A$'s columns being #cl[the $x$ values and a column of ones][h:two columns]. Compare the #lin(rel: "related")[singular value decomposition], which solves it when $A^T A$ is not invertible.`,
    ["Least squares", "Normal equations"],
    14,
    0.6,
  ),

  // Calculus (Typst and LaTeX)
  calculus(
    "typst",
    `= Derivatives

$ d/(d x) x^n = #cl[$n x^(n-1)$] $
$ d/(d x) sin x = #cl[$cos x$] #h(2em) d/(d x) cos x = #cl[$-sin x$] $
$ d/(d x) e^x = #cl[$e^x$] #h(2em) d/(d x) ln x = #cl[$1/x$] $`,
    ["Derivative", "Differentiation"],
    90,
    0.9,
  ),
  calculus(
    "typst",
    `= Chain rule

If $h(x) = f(g(x))$, then
$ h'(x) = #cl[$f'(g(x)) dot g'(x)$] $

For example, $d/(d x) sin(x^2) = #cl[$2 x cos(x^2)$]$, by #lin[derivatives].`,
    ["Chain rule"],
    84,
  ),
  calculus(
    "latex-note",
    `\\section*{Integration by parts}

\\[ \\int u \\, dv = \\begin{cl}uv - \\int v \\, du\\end{cl} \\]

It comes from integrating the \\begin{cl}[h:which rule?]product rule\\end{cl}.
A rule of thumb for choosing $u$ is \\begin{cl}[tag:hard]LIATE\\end{cl}: logarithms, inverse trigonometric, algebraic, trigonometric, exponential.

For example, $\\int x e^x \\, dx = \\begin{cl}(x - 1) e^x + C\\end{cl}$.`,
    ["Integration by parts"],
    75,
    0.6,
  ),
  calculus(
    "latex-note",
    `\\section*{Fundamental theorem of calculus}

If $f$ is continuous on $[a, b]$ and $F' = f$, then
\\[ \\int_a^b f(x) \\, dx = \\begin{cl}F(b) - F(a)\\end{cl}. \\]

Conversely, $\\frac{d}{dx} \\int_a^x f(t) \\, dt = \\begin{cl}f(x)\\end{cl}$: \\li{integration} and \\li{differentiation} are inverse operations.`,
    ["Fundamental theorem of calculus", "FTC"],
    66,
  ),
  calculus(
    "typst",
    `= Taylor series

The Taylor series of $f$ around $a$ is
$ f(x) = #cl[$sum_(n=0)^infinity (f^((n))(a)) / n! (x - a)^n$][tag:hard] $

With $a = 0$:
$ e^x = #cl[$sum_(n=0)^infinity x^n / n!$] #h(2em) sin x = #cl[$x - x^3/3! + x^5/5! - dots.c$] $`,
    ["Taylor series", "Maclaurin series"],
    48,
    0.5,
  ),
  calculus(
    "latex-note",
    `\\section*{L'H\\^opital's rule}

If $f(x)$ and $g(x)$ both tend to \\begin{cl}$0$, or both to $\\pm\\infty$\\end{cl}, as $x \\to a$, then
\\[ \\lim_{x \\to a} \\frac{f(x)}{g(x)} = \\begin{cl}\\lim_{x \\to a} \\frac{f'(x)}{g'(x)}\\end{cl} \\]
when the limit on the right exists.

For example, $\\lim_{x \\to 0} \\frac{\\sin x}{x} = \\begin{cl}1\\end{cl}$.`,
    ["L'Hopital's rule"],
    26,
  ),
  calculus(
    "typst",
    `= Integration

$ integral x^n dif x = #cl[$x^(n+1) / (n+1) + C$], quad n != -1 $
$ integral 1/x dif x = #cl[$ln |x| + C$] $
$ integral e^(a x) dif x = #cl[$1/a e^(a x) + C$] $

Integration undoes #lin[differentiation]: see the #lin(rel: "related")[fundamental theorem of calculus].`,
    ["Integration", "Antiderivative"],
    10,
  ),
  calculus(
    "latex-note",
    `\\section*{Product rule}

If $f$ and $g$ are differentiable, then
\\[ (f g)' = \\begin{cl}f' g + f g'\\end{cl}. \\]

It is also called the \\begin{cl}[t]Leibniz\\end{cl} rule. Integrating both sides gives \\li{integration by parts}, and with \\li{derivatives} of powers, $(x^2 \\sin x)' = \\begin{cl}2 x \\sin x + x^2 \\cos x\\end{cl}$.`,
    ["Product rule", "Leibniz rule"],
    42,
  ),
  calculus(
    "latex-note",
    `\\section*{The derivative of $x^2$ from the definition}

\\textbf{Claim.} For $f(x) = x^2$, $f'(x) = \\begin{cl}2x\\end{cl}$.

\\textbf{Proof.} By the definition of the \\li{derivative},
\\[ f'(x) = \\lim_{h \\to 0} \\frac{f(x + h) - f(x)}{h} = \\lim_{h \\to 0} \\frac{\\begin{cl}[h:expand the square]2 x h + h^2\\end{cl}}{h}. \\]
Since $h \\neq 0$ inside the limit, we may divide by it. Hence
\\[ f'(x) = \\lim_{h \\to 0} \\begin{cl}(2x + h)\\end{cl} = 2x. \\]`,
    ["Derivative from the definition"],
    20,
    0.8,
  ),
  // Created before the note it builds on, as notes written together often are: with
  // `order_by_links`, Riemann sums is learned first, and `new_after_linked_notes` holds this back
  // until it is
  calculus(
    "typst",
    `= Arc length

The length of the graph of $f$ from $a$ to $b$ is
$ L = #cl[$integral_a^b sqrt(1 + f'(x)^2) dif x$][tag:hard] $
the limit of #lin[Riemann sums] of the lengths of short straight pieces, each $sqrt(Delta x^2 + Delta y^2)$ long, which needs #lin[the derivative] of $f$.`,
    ["Arc length"],
    0,
  ),
  calculus(
    "typst",
    `= Riemann sums

Cut $[a, b]$ into $n$ pieces of width $Delta x = (b - a) / n$, and pick a point $x_i^*$ in each. Then
$ integral_a^b f(x) dif x = #cl[$lim_(n -> infinity) sum_(i=1)^n f(x_i^*) Delta x$] $
for every continuous $f$. With $x_i^*$ the left end of each piece it is the #cl[left] Riemann sum. See #lin(rel: "related")[integration].`,
    ["Riemann sums", "Riemann sum"],
    0,
  ),

  // Science, in a tag hierarchy three levels deep (the plant cell's image occlusion is in OCCLUSIONS)
  genetics(
    `# DNA base pairs

In DNA, adenine pairs with {{[r] thymine}} and guanine with {{[r] cytosine}}.

RNA has {{uracil}} in place of thymine. A–T pairs are held by {{[h:how many?] two}} hydrogen bonds, and G–C pairs by three.`,
    ["DNA", "Base pairs"],
    76,
    0.85,
  ),
  genetics(
    `# Mitosis

The phases of mitosis, in order: {{[ov] prophase}}, {{[ov] metaphase}}, {{[ov] anaphase}}, {{[ov] telophase}}.

Mitosis makes two cells with {{the same number of chromosomes}} as the parent, while meiosis makes {{[h:how many?] four}} cells with half as many. Both happen in the {{nucleus}} of a cell (see [organelles][li]).`,
    ["Mitosis", "Cell division"],
    46,
  ),
  // Nested clozes: one card hides the whole sentence, another only the word inside it. And a
  // link to a keyword no note has yet (the graph's "Unmatched links")
  cells(
    `# Cellular respiration

{{Cellular respiration turns glucose and oxygen into carbon dioxide, water and {{ATP}}}}, the cell's store of energy.

It happens mostly in the {{[h:organelle] mitochondria}}. Compare [photosynthesis][li], which runs it in reverse in the [chloroplasts][li].`,
    ["Cellular respiration", "ATP"],
    39,
  ),
  cells(
    `# Photosynthesis

6 CO~2~ + 6 H~2~O + light -> {{glucose}} (C~6~H~12~O~6~) + 6 O~2~

It takes place in the {{chloroplasts}}, whose green pigment, {{chlorophyll}}, absorbs mostly {{[g:1] red}} and {{[g:1] blue}} light. The [cellular respiration][li] of the same cells uses the glucose.`,
    ["Photosynthesis"],
    34,
  ),
  chemistry(
    `= Ideal gas law

$ #cl[$P V = n R T$] $

with $R approx #cl[$8.314$][h:in J/(mol K)]$ J/(mol K), and $T$ in #cl[kelvin][t].

At constant temperature, halving the volume #cl[doubles the pressure], which is #cl[Boyle's][h:whose law?] law.`,
    ["Ideal gas law", "Boyle's law"],
    59,
  ),
  chemistry(
    `= The mole

One mole holds #cl[$6.022 times 10^23$] particles, Avogadro's number. A mole of carbon-12 weighs exactly #cl[12 g].

The number of moles in $m$ grams of a substance of molar mass $M$ is #cl[$n = m / M$]. It is the $n$ of the #lin[ideal gas law].`,
    ["Mole", "Avogadro's number"],
    53,
    0.8,
  ),
  // Emphasis on the back (`be:`): the answer is highlighted after it is revealed
  chemistry(
    `= pH

$ "pH" = #cl[$-log_10 [H^+]$][be:] $

At 25 °C, pure water has a pH of #cl[7][be:]. Below it a solution is #cl[acidic][g:1], above it #cl[basic][g:1].

A pH lower by 1 means #cl[ten times as many $H^+$ ions].`,
    ["pH", "Acids and bases"],
    16,
  ),
  physics(
    `\\section*{Newton's second law}

\\[ F = \\begin{cl}m a\\end{cl} \\]

More generally, force is the rate of change of momentum $p = m v$: $F = \\begin{cl}\\frac{dp}{dt}\\end{cl}$, the \\li{derivative} of momentum.

The unit of force, $1\\,\\mathrm{kg\\,m/s^2}$, is the \\begin{cl}[t]newton\\end{cl}.`,
    ["Newton's second law", "Momentum"],
    57,
    0.8,
  ),
  physics(
    `\\section*{Kinetic energy}

A mass $m$ moving at speed $v$ has kinetic energy $E_k = \\begin{cl}\\frac{1}{2} m v^2\\end{cl}$.

The work done by a force along a path is $W = \\begin{cl}\\int F \\, dx\\end{cl}$, an \\li{integral}, and by the \\begin{cl}[h:which theorem?]work--energy theorem\\end{cl} it equals the change in kinetic energy. Doubling the speed multiplies the kinetic energy by \\begin{cl}4\\end{cl}.`,
    ["Kinetic energy", "Work-energy theorem"],
    29,
  ),
  // Every planet hidden on the front, and only the one asked for shown on the back (`f:all`, `b:a`)
  astronomy(
    `# Planets of the solar system

From the Sun outwards: {{[f:all;b:a] Mercury}}, {{[f:all;b:a] Venus}}, {{[f:all;b:a] Earth}}, {{[f:all;b:a] Mars}}, {{[f:all;b:a] Jupiter}}, {{[f:all;b:a] Saturn}}, {{[f:all;b:a] Uranus}}, {{[f:all;b:a] Neptune}}.

The largest is {{Jupiter}}, and the four nearest the Sun are {{rocky}}.`,
    ["Planets", "Solar system"],
    49,
  ),

  // History and geography, with their sources as keywords
  // `hide:`: in the other cards, the dissolution of the Soviet Union is hidden without being
  // asked, since it would give the year away
  history(
    `# The end of the Cold War

The Berlin Wall fell on 9 November {{1989}}, and Germany was reunified in {{[h:year] 1990}}.

The Soviet Union dissolved in December {{[g:*;hide:; g:ussr] 1991}}.`,
    ["Berlin Wall", "Cold War", "World history lecture 11"],
    73,
  ),
  history(
    `# The French Revolution

It began in {{1789}}, with the storming of the {{Bastille}} on 14 July, now France's national day.

Its motto was "{{[ov] Liberté}}, {{[ov] égalité}}, {{[ov] fraternité}}".`,
    ["French Revolution", "World history lecture 7"],
    62,
  ),
  history(
    `# The printing press

Johannes {{Gutenberg}} built a printing press with movable metal type in {{[h:city] Mainz}}, around {{[h:century] 1450}}.

His best-known book is the {{[t] Bible}}.`,
    ["Printing press", "Gutenberg", "World history lecture 5"],
    44,
  ),
  history(
    `# The Moon landing

Apollo 11 landed on the Moon on 20 July {{1969}}. {{[g:1] Neil Armstrong}} and {{[g:1] Buzz Aldrin}} walked on it, while {{Michael Collins}} stayed in orbit.`,
    ["Moon landing", "Apollo 11", "World history lecture 12"],
    11,
  ),
  // Reverse cards, with a tag on a forward card only (`tag:`) and on a reverse card only (`rtag:`)
  geography(
    `# Capitals

- Australia: {{[r;tag:hard] Canberra}}
- Canada: {{[r] Ottawa}}
- Brazil: {{[r] Brasília}}
- Turkey: {{[r;rtag:hard] Ankara}}`,
    ["Capitals"],
    79,
  ),
  geography(
    `# Oceans

The largest and deepest ocean is the {{Pacific}}. Its deepest point, in the {{[h:trench] Mariana Trench}}, is about {{[tag:hard] 11,000 m}} below sea level.

The second largest is the {{Atlantic}}.`,
    ["Oceans", "Mariana Trench"],
    54,
  ),
  geography(
    `# Mountains

The highest mountain, {{Everest}}, is {{8,849 m}} high, on the border of {{[g:1] Nepal}} and {{[g:1] China}}. The highest outside Asia is {{Aconcagua}}, in the Andes.`,
    ["Mountains", "Everest"],
    23,
  ),

  // Music, with audio clips uploaded with the notes: sine tones, made with
  //   ffmpeg -f lavfi -i "sine=frequency=440:duration=1.5" \
  //     -af "afade=t=in:d=0.02,afade=t=out:st=1.3:d=0.2" -ac 1 -b:a 48k a440.mp3
  // and fifth.mp3 the same with a second input at 660 Hz, mixed with `amix=inputs=2`
  music(
    `# Concert pitch

@audio(a440.mp3)

The A above middle C, A4, is tuned to {{440 Hz}}. The A an octave higher, A5, is {{880 Hz}}: an octave {{doubles}} the frequency.`,
    ["Concert pitch", "A440", "Octave"],
    47,
  ),
  music(
    `# Perfect fifth

@audio(fifth.mp3)

A perfect fifth is two notes whose frequencies are in the ratio {{3:2}}, such as 440 Hz and {{660 Hz}}. In equal temperament it is {{[h:how many semitones?] 7}} semitones. See [concert pitch][li].`,
    ["Perfect fifth", "Intervals"],
    17,
  ),

  // Japanese and Chinese, with readings over the words (the markdown-language-tools parser)
  japanese(
    `# Studying every day

私は 毎日[まいにち] 日本語[にほんご]を 勉強[べんきょう]しています。

昨日[きのう]は {{[h:ate sushi] 寿司[すし]を 食[た]べました}}。

*I study Japanese every day. Yesterday I ate sushi.*`,
    ["毎日", "Studying every day"],
    64,
  ),
  japanese(
    `勉強[べんきょう]: {{[h:a noun, and a verb with する] study}}`,
    ["勉強"],
    58,
    0.8,
    // The usual vocabulary card: the word on the front, its reading only on the back
    { readings: "back" },
  ),
  japanese(
    `"To eat", in its dictionary form: {{[t] 食[た]べる}}

Typed as 食べる or as its reading, たべる.`,
    ["食べる"],
    40,
    0.65,
  ),
  japanese(
    `- water: {{[r] 水[みず]}}
- tea: {{[r] お 茶[ちゃ]}}
- coffee: {{[r] コーヒー}}`,
    ["Drinks", "飲み物"],
    21,
  ),
  chinese(
    `# Greetings

我在学习 中文[zhōngwén]。

- Hello: {{[r] 你好[nǐ hǎo]}}
- Thank you: {{[r] 谢谢[xièxie]}}
- Goodbye: {{[r] 再见[zàijiàn]}}`,
    ["你好", "Chinese greetings"],
    30,
    0.8,
  ),

  // Spanish
  spanish(
    `<!--- # tts: es_ES --->
# Greetings

- Good morning: {{[r;tts:es_ES] buenos días}}
- Good afternoon: {{[r;tts:es_ES] buenas tardes}}
- See you later: {{[r;tts:es_ES] hasta luego}}`,
    ["Greetings", "Saludos"],
    81,
    0.9,
  ),
  spanish(
    `# Ser and estar

Both mean "to be". {{Ser}} is used for identity, origin and time, and {{estar}} for location and temporary states.

- Soy de México: "I {{[t:ci] am}} from Mexico" uses {{[h:ser or estar?] ser}}.
- Estoy cansado: "I am tired" uses {{estar}}.`,
    ["Ser", "Estar"],
    77,
    0.7,
  ),
  spanish(
    `# Preterite of *tener*

| | tener |
|---|---|
| yo | {{[t] tuve}} |
| tú | {{[t] tuviste}} |
| él / ella | {{[t] tuvo}} |
| nosotros | {{[t;tag:hard] tuvimos}} |
| ellos | {{[t;tag:hard] tuvieron}} |`,
    ["Tener", "Preterite", "Irregular verbs"],
    52,
    0.55,
  ),
  spanish(
    `# Food

- apple: {{[r;tts:es_ES] la manzana}}
- bread: {{[r;tts:es_ES] el pan}}
- water: {{[r;tts:es_ES] el agua}}
- cheese: {{[r;tts:es_ES] el queso}}`,
    ["Food", "Comida"],
    33,
  ),
  spanish(
    `# Por and para

- {{Para}} is used for a purpose or a destination: *Estudio {{[g:1;h:por or para?] para}} aprender.*
- {{Por}} is used for a cause, a duration or a means: *Gracias {{[g:2;h:por or para?] por}} todo.*`,
    ["Por", "Para"],
    15,
    0.5,
  ),
  spanish(
    `# Days of the week

{{[ov] lunes}}, {{[ov] martes}}, {{[ov] miércoles}}, {{[ov] jueves}}, {{[ov] viernes}}, {{[ov] sábado}}, {{[ov] domingo}}`,
    ["Days of the week", "Días de la semana"],
    1,
  ),
  // Reverse-only cards (`ro`): each shows the Spanish and asks for the English
  spanish(
    `# Animals

- el perro: {{[ro;tts:es_ES] the dog}}
- el gato: {{[ro;tts:es_ES] the cat}}
- el pájaro: {{[ro;tts:es_ES] the bird}}`,
    ["Animals", "Animales"],
    37,
    0.9,
  ),
  // A suspended cloze (`s`) in a note whose other cards are studied
  spanish(
    `# In the kitchen

- the knife: {{[r;tts:es_ES] el cuchillo}}
- the spoon: {{[r;tts:es_ES] la cuchara}}
- the frying pan: {{[s;r;tts:es_ES] la sartén}}`,
    ["Kitchen", "Cocina"],
    19,
  ),
  // A leech: the gender of these nouns keeps being forgotten
  spanish(
    `# Nouns of the unexpected gender

- {{[h:el or la?] el}} problema
- {{[h:el or la?] la}} mano
- {{[h:el or la?] el}} día
- {{[h:el or la?] el}} mapa`,
    ["Noun gender", "Género"],
    83,
    0.12,
  ),
  spanish(
    `# Preterite of *ir* and *ser*

*Ir* and *ser* share their preterite, so "fui" means either "I went" or "I was".

| | ir / ser |
|---|---|
| yo | {{[t] fui}} |
| tú | {{[t] fuiste}} |
| él / ella | {{[t] fue}} |
| nosotros | {{[t;tag:hard] fuimos}} |
| ellos | {{[t] fueron}} |`,
    ["Ir", "Ser", "Preterite", "Irregular verbs"],
    9,
    0.65,
  ),
  {
    parser: "markdown",
    data: `# Subjunctive after *ojalá*

*Ojalá* ("hopefully") is always followed by the {{subjunctive}}: *Ojalá que {{[h:llover] llueva}} mañana.*`,
    tags: ["languages:spanish"],
    keywords: ["Subjunctive", "Ojalá"],
    ago: 0,
    suspended: true,
  },
];

/** An image occlusion note (see build.ts), drawn on `image`, a file in this directory */
export interface DemoOcclusion {
  image: string;
  width: number;
  height: number;
  /** The masks, each hiding a label of the image, with its cloze settings */
  masks: { label: string; x: number; y: number; width: number; height: number; settings?: string }[];
  /** SVG elements drawn over the image on every card, as the editor's markup layer holds them */
  markup?: string;
  /** The note's text, where `@occlusion` stands for the image occlusion's snippet */
  data: string;
  tags: string[];
  keywords: string[];
  ago: number;
  ease: number;
}

export const OCCLUSIONS: DemoOcclusion[] = [
  {
    image: "cell.png",
    width: 600,
    height: 500,
    masks: [
      { label: "Nucleus", x: 452, y: 60, width: 146, height: 36 },
      { label: "Mitochondrion", x: 452, y: 150, width: 146, height: 36 },
      { label: "Chloroplast", x: 452, y: 240, width: 146, height: 36 },
      { label: "Vacuole", x: 452, y: 330, width: 146, height: 36 },
      { label: "Cell wall", x: 452, y: 420, width: 146, height: 36 },
    ],
    data: "# Parts of a plant cell\n\nName each labelled part.\n\n@occlusion",
    tags: ["science:biology:cells"],
    keywords: ["Plant cell", "Organelles"],
    ago: 38,
    ease: 0.7,
  },
  // Hints over the masks, markup, and text clozes in the same cards as image clozes (`g:`)
  {
    image: "earth.png",
    width: 640,
    height: 460,
    masks: [
      { label: "Crust", x: 446, y: 62, width: 160, height: 36, settings: "h:thinnest" },
      { label: "Mantle", x: 446, y: 152, width: 160, height: 36, settings: "g:mantle" },
      { label: "Outer core", x: 446, y: 242, width: 160, height: 36, settings: "g:outer;h:liquid" },
      { label: "Inner core", x: 446, y: 332, width: 160, height: 36, settings: "h:solid" },
    ],
    markup:
      '<line x1="220" y1="52" x2="220" y2="138" stroke="#0f172a" stroke-width="2" stroke-dasharray="6 4"/>' +
      '<text x="228" y="100" font-size="15" fill="#0f172a">about 2,900 km</text>' +
      '<text x="20" y="448" font-size="15" fill="#475569">Not to scale</text>',
    data: `# Layers of the Earth

@occlusion

The {{[g:mantle] mantle}} is solid rock that flows very slowly, and makes up most of the Earth's volume. The {{[g:outer] outer core}} is liquid iron and nickel, whose currents make the Earth's magnetic field.`,
    tags: ["science:earth"],
    keywords: ["Layers of the Earth", "Earth's core"],
    ago: 24,
    ease: 0.75,
  },
];

/**
 * Notes made after the reviews, today, from the cards of earlier notes: `inh:` copies a card's
 * scheduling and reviews onto a new card, as when a note is split, and `mv:` moves the card itself,
 * as when notes are merged. `deletes` are the first keywords of the notes merged into it, deleted
 * once their cards have moved.
 */
export const LATER_NOTES: (DemoNote & { deletes?: string[] })[] = [
  {
    ...algorithms(
      `# Resizing a hash table

When the {{[inh:@card(Hash table/4)] load factor}} passes a threshold, such as 0.75, a [hash table][li] allocates a bigger array, usually {{twice}} as large, and {{rehashes}} every key into it.

One resize takes O(n), but it happens rarely enough that inserts are still O(1) {{amortized}}.`,
      ["Hash table resizing", "Rehashing"],
      0,
    ),
  },
  {
    ...algorithms(
      `# Stacks and queues

- A stack is {{[mv:@card(Stack/1)] LIFO}}, used for undo, recursion and [DFS][li].
- A queue is {{[mv:@card(Queue/1)] FIFO}}, used for buffers and [BFS][li].
- A deque allows both: push and pop at {{either end}}.`,
      ["Stacks and queues", "Stack", "Queue", "Deque"],
      0,
    ),
    deletes: ["Stack", "Queue"],
  },
];

/** Saved searches, synced with the collection */
export const SAVED_SEARCHES = [
  { name: "Hard cards", query: "tag=hard" },
  { name: "Rust, due this week", query: 'tag="programming:rust" and c.due<=today+7d' },
  { name: "Linear algebra and what builds on it", query: 'tag="math:linear-algebra" or dependent_of=(tag="math:linear-algebra")' },
  { name: "Leeches and cards I keep burying", query: "tag=leech or tag=buried" },
  { name: "Forgotten this week", query: "c.rated[>=today-7d]=1" },
  { name: "Least likely to recall", query: "sort_by_asc=c.retrievability limit=20" },
  { name: "Links in a loop", query: "in_link_cycle" },
];
