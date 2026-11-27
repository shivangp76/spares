#let se(..f) = [[#text(fill: olive, ..f)]] // settings
// `rel` says which way the link points: "prerequisite" (the default), "dependent" or "related"
#let lin(keyword, note_link: none, rel: "prerequisite") = if (
  rel not in ("prerequisite", "dependent", "related")
) { panic("rel: of #lin must be \"prerequisite\", \"dependent\" or \"related\", not " + repr(rel)) } else if note_link != none [
  [#link(note_link, keyword)] // linked note
] else [
  #text(fill: green, keyword)
]
#let blank = "_____" // one word blank
#let blanks = "__________" // multiple words blank
// #let cl(body, ..opts) = body
#let cl(body, ..opts) = [[#body]]
// #let cl(body, ..opts) = { "[("; opts.pos().join(""); ") "; body; "]" }
#let cloze(hint: none, to_answer: true, id: none) = if to_answer != true {
  [#highlight(fill: orange)[#blank#[(no answer)]]]
} else {
  if id != none {
    h(0pt, weak: true)
    box(rotate(90deg, text(fill: rgb("00000000"), size: 1pt, id)))
  }
  if hint == none [
    [#highlight[#blank]]
  ] else [
    [#highlight[#blank#[(Hint: #hint)]]]
  ]
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
// A revealed cloze is highlighted inline when it fits in a line of text, so the clozes of a group
// stay in their sentence. A block holds what can't sit in a paragraph (display math, lists,
// paragraphs, environments).
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
    highlight(fill: aqua, markup)
  }
}

// spares: body
