// mdedit's built-in Typst template (SPEC 10). A template's own
// `template.typ` replaces this file; copy it as a starting point.
//
// Contract: the generated `main.typ` does
//
//   #import "/__mdedit/template.typ": *
//   #show: md-document
//
// then writes the body with plain Typst markup (headings with labels,
// paragraphs, #emph, #strong, #strike, #link, #footnote, #quote, #list,
// #enum, #linebreak) and calls the functions below. The template options
// from template.toml are the `conf` dictionary in "/__mdedit/conf.typ".
//
//   md-title(lead: content | none)[title]
//   md-toc()
//   md-figure(path, alt: str, caption: content | none)
//   md-image(path, alt: str)                 inline image
//   md-missing-image[alt]                    image file not found
//   md-callout(kind: str, title: content)[body]
//   md-code(lang: str, lines)                lines: array of arrays of
//                                            (text, "#rrggbb" | none, "b" | "i" | "bi" | "")
//   md-tasks(tight: bool, ..(checked: bool | none, content))
//   md-table(align: array, header: array, ..rows)
//   md-rule()
//   md-comment(author: str)[body]

#import "/__mdedit/conf.typ": conf

#let guide = conf.style == "guide"
#let report = conf.style == "report"

#let eyebrow(body, fill: conf.accent-text) = text(
  size: 0.75em,
  weight: 600,
  tracking: 0.08em,
  fill: fill,
  upper(body),
)

#let md-document(body) = {
  set document(title: conf.title)
  set page(
    paper: conf.paper,
    margin: conf.margin,
    header: if conf.header != (none, none, none) {
      set text(size: 0.8em, fill: conf.text-muted)
      grid(
        columns: (1fr, auto, 1fr),
        align(left, conf.header.at(0)), align(center, conf.header.at(1)), align(right, conf.header.at(2)),
      )
    },
    footer: if conf.footer != (none, none, none) {
      set text(size: 0.8em, fill: conf.text-muted)
      grid(
        columns: (1fr, auto, 1fr),
        align(left, conf.footer.at(0)), align(center, conf.footer.at(1)), align(right, conf.footer.at(2)),
      )
    },
  )
  set text(font: conf.font-body, size: conf.size, fill: conf.text, lang: "en")
  set par(leading: (conf.line-height - 1) * 1em + 0.2em, spacing: 1.1em, justify: report)
  show raw: set text(font: conf.font-mono, size: 0.9em)
  show raw.where(block: false): it => box(
    fill: conf.bg-subtle,
    inset: (x: 2pt, y: 0pt),
    outset: (y: 3pt),
    radius: 3pt,
    it,
  )
  show link: set text(fill: conf.accent-text)
  show footnote: set text(fill: conf.accent-text)
  set footnote.entry(separator: line(length: 30%, stroke: 0.5pt + conf.border))
  show footnote.entry: set text(size: 0.85em)

  // Headings: `01` numbers over a rule (Guide) or decimal numbers.
  set heading(numbering: if conf.heading-numbers {
    if conf.number-style == "guide" { (..n) => if n.pos().len() == 1 { numbering("01", ..n) } } else { "1.1" }
  })
  show heading: set text(font: conf.font-heading, fill: conf.text)
  show heading: it => {
    let size = (1.6em, 1.3em, 1.12em, 1em, 1em, 1em).at(it.level - 1)
    let weight = if it.level <= 2 { 700 } else { 600 }
    let above = if it.level == 1 { 2.4em } else { 1.6em }
    if guide and it.level == 1 {
      block(above: above, below: 0.9em, sticky: true, {
        line(length: 100%, stroke: 0.75pt + conf.border)
        v(1.2em)
        if it.numbering != none {
          text(font: conf.font-mono, size: 0.85em, fill: conf.accent, counter(heading).display(it.numbering))
          v(0.2em, weak: true)
        }
        text(size: 1.75em, weight: 700, tracking: -0.015em, it.body)
      })
    } else {
      block(above: above, below: 0.8em, sticky: true, {
        set text(size: size, weight: weight, tracking: if it.level <= 2 { -0.01em } else { 0em })
        if it.numbering != none and counter(heading).get().len() > 0 {
          let num = counter(heading).display(it.numbering)
          if num != none and num != [] {
            text(fill: if guide { conf.accent } else { conf.text-secondary }, num)
            h(0.6em)
          }
        }
        if it.level == 6 { text(fill: conf.text-secondary, it.body) } else { it.body }
      })
    }
  }

  // Lists: muted bullets; Guide numbers ordered lists in step badges.
  set list(marker: text(fill: conf.text-muted, sym.bullet), indent: 0.2em, body-indent: 0.6em)
  set enum(indent: 0.2em, body-indent: 0.6em, numbering: if guide {
    n => box(
      width: 1.6em,
      height: 1.6em,
      radius: 50%,
      fill: conf.accent-soft,
      align(center + horizon, text(size: 0.75em, weight: 600, fill: conf.accent-text, str(n))),
    )
  } else { n => text(fill: conf.text-secondary, str(n) + ".") })

  show quote.where(block: true): it => block(
    stroke: (left: 3pt + conf.border),
    inset: (left: 1em, y: 0.2em),
    text(fill: conf.text-secondary, it.body),
  )

  body
}

#let md-title(lead: none, title) = {
  let page-title = conf.title-page
  block(below: 1.2em, {
    if page-title { v(25%) }
    if conf.eyebrow != none {
      eyebrow(conf.eyebrow)
      v(0.6em, weak: true)
    }
    text(font: conf.font-heading, size: if page-title { 2.6em } else { 2.1em }, weight: 700, tracking: -0.02em, title)
    if lead != none {
      v(0.9em, weak: true)
      text(size: 1.2em, fill: conf.text-secondary, lead)
    }
    if page-title {
      v(1.5em)
      text(fill: conf.text-muted, conf.date)
    }
  })
  if page-title and not conf.toc { pagebreak(weak: true) }
}

#let md-toc() = {
  show outline.entry.where(level: 1): set text(weight: 600)
  outline(title: eyebrow([Contents]), indent: 1.2em)
  if conf.title-page { pagebreak(weak: true) } else { v(1em) }
}

#let md-figure(path, alt: "", caption: none) = figure(
  box(clip: true, radius: if guide { 8pt } else { 0pt }, image(path, alt: alt)),
  caption: if caption != none { text(size: 0.85em, fill: conf.text-muted, caption) },
  numbering: none,
  supplement: none,
  gap: 0.6em,
)

#let md-image(path, alt: "") = box(height: 1.2em, baseline: 0.2em, image(path, alt: alt, height: 1.2em))

#let md-missing-image(alt) = text(fill: conf.text-muted, [\[#alt\]])

#let md-callout(kind: "note", title: [], body) = {
  let warn = kind in ("warning", "caution", "important")
  let fill = if guide or warn { if warn { conf.warning-bg } else { conf.bg-subtle } } else { none }
  block(
    width: 100%,
    fill: fill,
    stroke: if guide { 0.75pt + conf.border } else { (left: 3pt + conf.accent) },
    radius: if guide { 8pt } else { 0pt },
    inset: (x: 1.4em, y: 1.1em),
    above: 1.4em,
    below: 1.4em,
    {
      eyebrow(title, fill: if warn { conf.warning-text } else { conf.accent-text })
      v(0.7em, weak: true)
      body
    },
  )
}

#let md-code(lang: "", lines) = block(
  width: 100%,
  fill: conf.bg-subtle,
  radius: if guide { 8pt } else { 4pt },
  inset: (x: 1.2em, y: 1em),
  above: 1.2em,
  below: 1.2em,
  {
    set par(justify: false, leading: 0.55em)
    set text(font: conf.font-mono, size: 0.85em, fill: conf.text-secondary)
    if lang != "" {
      place(top + right, dx: 0.6em, dy: -0.5em, eyebrow(lang, fill: conf.text-muted))
    }
    for (i, line) in lines.enumerate() {
      if i > 0 { linebreak() }
      for (t, color, flags) in line {
        let span = text(t)
        if color != none { span = text(fill: rgb(color), span) }
        if "b" in flags { span = strong(span) }
        if "i" in flags { span = emph(span) }
        span
      }
      if line.len() == 0 { [~] }
    }
  },
)

#let md-tasks(tight: true, ..items) = {
  let check(state) = if state == none {
    text(fill: conf.text-muted, sym.bullet)
  } else {
    box(
      width: 0.95em,
      height: 0.95em,
      baseline: 0.15em,
      radius: 2pt,
      stroke: 0.9pt + conf.accent,
      fill: if state { conf.accent } else { none },
      if state { align(center + horizon, text(size: 0.7em, fill: white, weight: 700, sym.checkmark)) },
    )
  }
  block(above: 1em, below: 1em, grid(
    columns: (1.6em, 1fr),
    row-gutter: if tight { 0.9em } else { 1.3em },
    ..items.pos().map(((state, body)) => (check(state), body)).flatten(),
  ))
}

#let md-table(align: (), header: (), ..rows) = {
  let rows = rows.pos()
  let n = header.len()
  let cells = (
    table.header(..header.map(h => eyebrow(h, fill: conf.text-secondary))),
    ..rows.map(r => r + (([],) * calc.max(0, n - r.len()))).map(r => r.slice(0, n)).flatten(),
  )
  let tbl = table(
    columns: (auto,) * calc.max(0, n - 1) + (1fr,),
    align: (x, y) => if align.len() > x and align.at(x) != auto { align.at(x) } else { left },
    inset: (x: 1em, y: 0.75em),
    stroke: (x, y) => if y > 0 { (top: 0.75pt + conf.border) },
    fill: (x, y) => if y == 0 and not guide { conf.bg-subtle },
    ..cells,
  )
  if guide {
    block(width: 100%, stroke: 0.75pt + conf.border, radius: 8pt, clip: true, above: 1.2em, below: 1.2em, tbl)
  } else {
    block(width: 100%, stroke: (top: 0.75pt + conf.border, bottom: 0.75pt + conf.border), above: 1.2em, below: 1.2em, tbl)
  }
}

#let md-rule() = block(above: 2em, below: 2em, line(length: 100%, stroke: 0.75pt + conf.border))

#let md-comment(author: "", body) = footnote[#text(weight: 600, author): #body]
