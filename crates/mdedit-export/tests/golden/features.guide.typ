// Template options, generated from template.toml.
#let conf = (
  style: "guide",
  paper: "a4",
  margin: (top: 22mm, right: 22mm, bottom: 22mm, left: 22mm),
  font-body: "Hanken Grotesk",
  font-heading: "Hanken Grotesk",
  font-mono: "JetBrains Mono",
  size: 10.5pt,
  line-height: 1.6,
  accent: rgb("#d9542b"),
  text: rgb("#1b1a19"),
  text-secondary: rgb("#5f5b57"),
  text-muted: rgb("#716d68"),
  accent-text: rgb("#b8431f"),
  accent-soft: rgb("#fbe9e2"),
  border: rgb("#e3dfda"),
  bg-subtle: rgb("#f5f3f1"),
  warning-bg: rgb("#fbf0d9"),
  warning-text: rgb("#8a5a00"),
  heading-numbers: true,
  number-style: "guide",
  title-page: false,
  toc: false,
  title: "Feature tour",
  date: "2026-01-02",
  eyebrow: none,
  header: (none, none, none),
  footer: ([Feature tour], none, [#context counter(page).display() / #context counter(page).final().first()]),
  header-style: (logo: none, logo-pos: "left", logo-height: 8mm, fill: rgb("#716d68"), size: 8.4pt, font: "Hanken Grotesk", weight: "regular", rule: false),
  footer-style: (logo: none, logo-pos: "left", logo-height: 8mm, fill: rgb("#716d68"), size: 8.4pt, font: "Hanken Grotesk", weight: "regular", rule: false),
)

// ---- main.typ
#import "/__mdedit/template.typ": *
#show: md-document

#md-title(lead: [Lead paragraph with #md-comment(author: "Ann")[A review comment.]#emph[emphasis], #strong[strong], #strike[strike] and `code`.], )[Feature tour]

= Text that needs escaping <text-that-needs-escaping>

Typst specials: \#hash \$dollar #emph[star] #emph[under] \[brackets\] \@at \~tilde \\backslash
\// not a comment, \-- not a dash, #raw("a`b"), 3 \< 4 \> 2.

#list(tight: true,
  [not a list item],
)

#enum(start: 12, tight: true,
  [not an enumeration
\= not a heading],
)

Link to #link(<lists-and-tasks>)[a heading], a missing anchor and
#link("https://example.com/a_b?c=d")[the web];.#footnote[A footnote with #emph[style];.] <fn-note>

= Lists and tasks <lists-and-tasks>

#list(tight: true,
  [one],
  [two

#list(tight: true,
  [nested #strong[bold]],
)],
  [three],
)

#enum(start: 3, tight: true,
  [three],
  [four],
)

#md-tasks(tight: true,
  (false, [open task]),
  (true, [done task]),
  (none, [plain item in a task list]),
)

#enum(start: 1, tight: false,
  [Loose item.

Second paragraph of the loose item.],
  [Another loose item.],
)

= Blocks <blocks>

#quote(block: true)[A plain quote
over two lines.]

#md-callout(kind: "note", title: [NOTE])[A GitHub-style note.]

#md-callout(kind: "note", title: [Remember])[A callout with a bold title.]

#md-code(lang: "python", (
  (("def", "#b8431f", ""), (" ", none, ""), ("greet", "#7a4fb0", ""), ("(", none, ""), ("name", "#1b1a19", ""), ("):", none, ""), ),
  (("    ", none, ""), ("return", "#b8431f", ""), (" ", none, ""), ("f", "#b8431f", ""), ("\"hi ", "#376e44", ""), ("{name}", none, ""), ("\"", "#376e44", ""), ("  ", none, ""), ("# comment", "#716d68", "i"), ),
))

#md-code(lang: "", (
  (("plain text block", none, ""), ),
))

#md-table(
  align: (left, center, right, ),
  header: ([Left], [Centre], [Right], ),
  ([a], [b], [c], ),
  ([`x`], [#strong[y]], [], ),
)

#md-figure("/__img/missing.png", alt: "Missing image")

Inline #md-image("/__img/diagram.png", alt: "icon") image.

#md-rule()

Last paragraph#footnote(<fn-note>) with a hard#linebreak()
break.
