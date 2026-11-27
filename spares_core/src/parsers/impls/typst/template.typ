// One page as tall as the note. The web app passes its width in CSS px; 1pt = 4/3 CSS px, so text
// shows at its true size. Capped so wide screens, and PDFs rendered without the width, keep lines
// a readable length. The web app already pads around the page, so the margin is small.
#let _spares-width = sys.inputs.at("spares-width", default: none)
#set page(
  width: if _spares-width == none { 8in } else { calc.min(8in, float(_spares-width) * 0.75pt) },
  height: auto,
  margin: if _spares-width == none { 10pt } else { 5pt },
)
#let se(..f) = [[#text(fill: olive, ..f)]] // settings
// `rel` says which way the link points: "prerequisite" (the default), "dependent" or "related"
#let lin(keyword, note_link: none, rel: "prerequisite") = if (
  rel not in ("prerequisite", "dependent", "related")
) { panic("rel: of #lin must be \"prerequisite\", \"dependent\" or \"related\", not " + repr(rel)) } else if note_link != none {
  [[#link(note_link, keyword)]] // linked note
} else {
  text(fill: green, keyword)
}
#let blank = "_____" // one word blank
#let blanks = "__________" // multiple words blank
// #let cl(body, ..opts) = body
#let cl(body, ..opts) = [[#body]]
// #let cl(body, ..opts) = { "[("; opts.pos().join(""); ") "; body; "]" }
#let cloze(hint: none, to_answer: true, id: none) = if to_answer != true {
  [#highlight(fill: orange)[#blank (no answer)]]
} else {
  if id != none {
    h(0pt, weak: true)
    box(rotate(90deg, text(fill: rgb("00000000"), size: 1pt, id)))
  }
  if hint == none [[#highlight[#blank]]] else [[#highlight[#blank (Hint: #hint)]]]
}
#let _space = [ ].func()
// Whether content can't sit inside a paragraph.
#let _is-block-content(c) = {
  if type(c) != content { return false }
  let f = c.func()
  // The most common children, checked first: this saves about a third of the time on a long cloze.
  if f == text or f == _space { return false }
  if f in (parbreak, block, list.item, enum.item, terms.item, figure, table, grid, heading, place, align, pagebreak) { return true }
  if f == math.equation { return c.block }
  if f == raw { return c.block }
  if c.has("children") { return c.children.any(_is-block-content) }
  if c.has("child") { return _is-block-content(c.child) }
  if c.has("body") { return _is-block-content(c.body) }
  false
}
// The equation that is all of `c` but spaces, if there is one.
#let _only-equation(c) = {
  if type(c) != content { return none }
  if c.func() == math.equation { return c }
  if c.has("children") {
    let rest = c.children.filter(x => x.func() != _space)
    if rest.len() == 1 { return _only-equation(rest.first()) }
  }
  none
}
#let _reveal-box = box.with(fill: aqua, outset: (y: .2em))
// A cloze that is all math (see `cloze-reveal`) in display style in display math. A show rule,
// unlike a `state`, costs no extra layout pass.
#show math.equation.where(block: true): it => {
  show <spares-math-cloze>: b => _reveal-box(math.equation(math.display(b.body.body)))
  it
}
// A revealed cloze is highlighted inline when it fits in a line of text, so the clozes of a group
// stay in their sentence. A block holds what can't sit in a paragraph (display math, lists,
// paragraphs, environments).
// `highlight` colours only text, so math gets a box. A cloze that is all math, as clozes in an
// equation are, is one box, laid out in display style in display math: its own `$...$` would
// otherwise be inline, with small fractions and sums.
// The invisible `id` is what the web app scrolls to. Inline, it takes no width and keeps the space
// before the cloze.
#let cloze-reveal(id: none, markup) = {
  let id-mark = rotate(90deg, text(fill: rgb("00000000"), size: 1pt, id))
  if _is-block-content(markup) {
    if id != none {
      h(0pt, weak: true)
      box(id-mark)
    }
    block(fill: aqua, outset: .2em, markup)
  } else {
    if id != none { box(width: 0pt, id-mark) }
    let equation = _only-equation(markup)
    if equation != none [#_reveal-box(equation)<spares-math-cloze>] else {
      show math.equation: _reveal-box
      highlight(fill: aqua, markup)
    }
  }
}

// spares: body
