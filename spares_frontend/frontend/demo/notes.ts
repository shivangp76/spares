// The demo collection's notes (see build.ts). Written for the demo, on general topics, so that it
// shows each feature the README lists: the three parsers, clozes and their settings, links
// between notes, keywords, a tag hierarchy, card tags, text to speech, typed answers, an image
// occlusion, and the markdown-language-tools parser's readings (dictionary lookup needs a host,
// which the demo hasn't). Each note's `ago` is how many days before the build it was created, which its review
// history starts from.

export type ParserName = "markdown" | "typst" | "latex-note" | "markdown-language-tools";

export interface DemoNote {
  parser: ParserName;
  data: string;
  tags: string[];
  keywords?: string[];
  /** Days before the build it was created. Notes created today have no reviews. */
  ago: number;
  /** How well its cards are known, from 0 (often forgotten) to 1 (never), for the made-up reviews */
  ease?: number;
  suspended?: boolean;
  /** The note's custom data, such as `{"language": "ja"}` for markdown-language-tools */
  customData?: Record<string, unknown>;
}

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
  rust(
    `# Lifetimes

\`fn longest<'a>(x: &'a str, y: &'a str) -> &'a str\` says that the returned reference lives {{[tag:hard] at least as long as the shorter of \`x\` and \`y\`}}.

Lifetimes are part of [borrowing][li]: they only describe how long references are valid, they never {{change how long values live}}.`,
    ["Lifetimes"],
    64,
    0.55,
  ),
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

With a binary heap it takes {{[tag:hard] O((V + E) log V)}}. With negative weights, use {{Bellman–Ford}} instead. It is [BFS][li] with a priority queue in place of the queue.`,
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
  linearAlgebra(
    `= Rank–nullity theorem

For a linear map $T: V -> W$ with $V$ finite-dimensional,
$ dim V = #cl[$"rank"(T) + "nullity"(T)$] $
where the rank is the dimension of the #cl[image] and the nullity that of the #cl[kernel].`,
    ["Rank-nullity theorem"],
    65,
    0.55,
  ),
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

/** The image occlusion note (see build.ts), on biology/cell.png: its masks hide these labels. */
export const CELL_LABELS: { label: string; x: number; y: number; width: number; height: number }[] = [
  { label: "Nucleus", x: 452, y: 60, width: 146, height: 36 },
  { label: "Mitochondrion", x: 452, y: 150, width: 146, height: 36 },
  { label: "Chloroplast", x: 452, y: 240, width: 146, height: 36 },
  { label: "Vacuole", x: 452, y: 330, width: 146, height: 36 },
  { label: "Cell wall", x: 452, y: 420, width: 146, height: 36 },
];

/** Saved searches, synced with the collection */
export const SAVED_SEARCHES = [
  { name: "Hard cards", query: "tag=hard" },
  { name: "Rust, due this week", query: 'tag="programming:rust" and c.due<=today+7d' },
  { name: "Linear algebra and what builds on it", query: 'tag="math:linear-algebra" or dependent_of=(tag="math:linear-algebra")' },
];
