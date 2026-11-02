// Template options, generated from template.toml.
#let conf = (
  style: "plain",
  paper: "a4",
  margin: (top: 25mm, right: 20mm, bottom: 25mm, left: 20mm),
  font-body: "Hanken Grotesk",
  font-heading: "Hanken Grotesk",
  font-mono: "JetBrains Mono",
  size: 11pt,
  line-height: 1.5,
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
  heading-numbers: false,
  number-style: "decimal",
  title-page: false,
  toc: false,
  title: "Off-ramp stablecoins to a bank",
  date: "2026-01-02",
  eyebrow: none,
  header: (none, none, none),
  footer: (none, [#context counter(page).display()], none),
  header-style: (logo: none, logo-pos: "left", logo-height: 8mm, fill: rgb("#716d68"), size: 8.8pt, font: "Hanken Grotesk", weight: "regular", rule: false),
  footer-style: (logo: none, logo-pos: "left", logo-height: 8mm, fill: rgb("#716d68"), size: 8.8pt, font: "Hanken Grotesk", weight: "regular", rule: false),
)

// ---- main.typ
#import "/__mdedit/template.typ": *
#show: md-document

#md-title()[Off-ramp stablecoins to a bank]

A step-by-step guide for operators turning a customer's stablecoins into a
bank payment. It covers opening a wallet, receiving funds, checking the
balance, and paying a beneficiary.

= How the off-ramp works <how-the-off-ramp-works>

An off-ramp takes #strong[four steps#md-comment(author: "Ann")[A review comment.]]: open a managed wallet, send stablecoins to
it, check the balance, then pay out. The partner holds the wallet and
converts the stablecoins to #emph[fiat] when the payout leaves.#footnote[The partner is the regulated entity that holds the funds.] <fn-partner>

#md-figure("/__img/diagram.png", alt: "Flow of funds", caption: [Funds move from the wallet to the bank.])

Before you start, check that:

#md-tasks(tight: true,
  (true, [The customer is #strong[Verified];.]),
  (false, [You know which stablecoin the customer will send.]),
  (false, [You have the beneficiary's bank details.]),
)

#md-callout(kind: "note", title: [If you can't add the wallet])[No #emph[Add wallet] card means the customer already has every network and
token on offer.]

= Send stablecoins to the wallet <send-stablecoins-to-the-wallet>

#enum(start: 1, tight: true,
  [On the #strong[Wallets] tab, click the card of the network.],
  [Copy the address with `Copy wallet address`.],
  [From the sending wallet, send the token on that network.],
)

#md-table(
  align: (left, center, right, auto, ),
  header: ([Asset], [Network], [Minimum], [Arrival], ),
  ([USDT], [Tron], [10,000], [3-5 days], ),
  ([USDC], [Ethereum], [10,000], [3-5 days], ),
)

== Check the balance by API <check-the-balance-by-api>

#md-code(lang: "rust", (
  (("fn", "#b8431f", ""), (" ", none, ""), ("balance", "#7a4fb0", ""), ("(", none, ""), ("wallet", "#1b1a19", ""), (": &Wallet) -> Result<", none, ""), ("u64", "#b8431f", ""), (", Error> {", none, ""), ),
  (("    ", none, ""), ("// Sum confirmed deposits only.", "#716d68", "i"), ),
  (("    ", none, ""), ("let", "#b8431f", ""), (" total = wallet.", none, ""), ("deposits", "#7a4fb0", ""), ("().", none, ""), ("filter", "#7a4fb0", ""), ("(|", none, ""), ("d", "#1b1a19", ""), ("| d.confirmed).", none, ""), ("map", "#7a4fb0", ""), ("(|", none, ""), ("d", "#1b1a19", ""), ("| d.amount).", none, ""), ("sum", "#7a4fb0", ""), ("();", none, ""), ),
  (("    ", none, ""), ("Ok", "#8a5a00", ""), ("(total)", none, ""), ),
  (("}", none, ""), ),
))

#md-callout(kind: "warning", title: [WARNING])[For a large amount, send a small test transfer first.]

#md-rule()

See #link(<how-the-off-ramp-works>)[the first section] or the
#link("https://example.com/docs")[Magma docs];. Edge cases: 50% \* 2 = 100, a\_b\_c,
\#hashtag, , \@mention, \~tilde\~\~ and a line#linebreak()
break.#footnote(<fn-partner>)
