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
#let cloze-reveal(id: none, markup) = {
  if id != none {
    h(0pt, weak: true)
    box(rotate(90deg, text(fill: rgb("00000000"), size: 1pt, id)))
  }
  block(fill: aqua, outset: .2em)[
    #markup
  ]
}

// spares: body
