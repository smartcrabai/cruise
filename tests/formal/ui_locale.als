// Static cross-check of the same finite precedence abstraction as ui_locale.qnt.
module ui_locale
abstract sig Locale {}
one sig English, Chinese, Spanish, French, Portuguese, Japanese, Korean extends Locale {}
one sig Inputs { env, config, system: lone Locale, effective: one Locale }
sig Browser { cookie: lone Locale, rendered: one Locale }

fun browserLocale[c: lone Locale, process: one Locale]: one Locale {
  some c => c else process
}
fact Resolution {
  Inputs.effective = (some Inputs.env => Inputs.env else
    (some Inputs.config => Inputs.config else
      (some Inputs.system => Inputs.system else English)))
  all b: Browser | b.rendered = browserLocale[b.cookie, Inputs.effective]
}
assert Precedence {
  (some Inputs.env implies Inputs.effective = Inputs.env)
  and (no Inputs.env and some Inputs.config implies Inputs.effective = Inputs.config)
  and (no Inputs.env and no Inputs.config and some Inputs.system implies Inputs.effective = Inputs.system)
  and (no Inputs.env and no Inputs.config and no Inputs.system implies Inputs.effective = English)
}
// Missing inputs abstract absent, automatic, and invalid locale values.
// A missing cookie also abstracts invalid or unparseable cookie values.
// String parsing, persistence, and correspondence to production are outside this model.
assert BrowserPrecedence {
  all b: Browser |
    (some b.cookie implies b.rendered = b.cookie) and
    (no b.cookie implies b.rendered = Inputs.effective)
}
assert BrowserIsolation {
  all disj a, b: Browser |
    (a.cookie = b.cookie implies a.rendered = b.rendered) and
    (a.rendered = browserLocale[a.cookie, Inputs.effective])
}
pred IndependentBrowsersWitness {
  some disj a, b, c: Browser |
    Inputs.env = Japanese and a.cookie = French and b.cookie = Spanish and no c.cookie
    and a.rendered = French and b.rendered = Spanish and c.rendered = Japanese
}
pred OverrideWitness {
  Inputs.env = Japanese and Inputs.config = French and Inputs.system = Spanish
  and Inputs.effective = Japanese
}
pred ConfigWitness {
  no Inputs.env and Inputs.config = French and Inputs.system = Spanish
  and Inputs.effective = French
}
pred SystemWitness {
  no Inputs.env and no Inputs.config and Inputs.system = Korean
  and Inputs.effective = Korean
}
pred DefaultWitness { no Inputs.env and no Inputs.config and no Inputs.system and Inputs.effective = English }
check Precedence for 8 expect 0
check BrowserPrecedence for 8 expect 0
check BrowserIsolation for 8 expect 0
run IndependentBrowsersWitness for 8 expect 1
run OverrideWitness for 8 expect 1
run ConfigWitness for 8 expect 1
run SystemWitness for 8 expect 1
run DefaultWitness for 8 expect 1
