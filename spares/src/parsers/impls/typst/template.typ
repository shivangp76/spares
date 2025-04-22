#let se(..f) = [[#text(fill: olive, ..f)]] // settings
#let lin(keyword, note_link: none) = if note_link != none [
  [#link(note_link, keyword)] // linked note
] else [
  #text(fill: green, keyword)
]
#let blank = "_____" // one word blank
#let blanks = "__________" // multiple words blank
// #let cl(body, ..opts) = body
#let cl(body, ..opts) = [[#body]]
// #let cl(body, ..opts) = { "[("; opts.pos().join(""); ") "; body; "]" }
#let cloze(hint: none, to_answer: true) = if to_answer != true [
  [#highlight(fill: orange)[#blank#[(no answer)]]]
] else if hint == none [
  [#highlight[#blank]]
] else [
  [#highlight[#blank#[(#hint)]]]
]

// spares: note body
