; Event-B syntax highlighting queries (tree-sitter).
;
; Hand-maintained alongside the structural grammar in grammar.js. Captures use
; the standard ecosystem names (@keyword, @operator, @constant.builtin,
; @function.builtin, @comment, @string, @number, @label, @variable,
; @punctuation.*).
;
; Pattern order matters: the tree-sitter highlight crate (Zed, the CLI) and
; nvim-treesitter give LATER patterns precedence, so the generic captures come
; first and the specific ones (@module, @type, @function, @variable.parameter)
; follow and override them.
;
; ASCII operator spellings are aliased to their canonical Unicode form in the
; grammar (e.g. `<=` parses as the anonymous token `≤`), so each operator
; needs only its canonical spelling here — which for the Rodin private-use
; arrows U+E100–E102 is the ASCII spelling, as Unicode has no equivalent.

(comment) @comment
(number) @number
(label) @label
(identifier) @variable

["(" ")" "[" "]" "{" "}"] @punctuation.bracket
"," @punctuation.delimiter

[
  "context"
  "machine"
  "extends"
  "sets"
  "constants"
  "axioms"
  "theorems"
  "refines"
  "sees"
  "variables"
  "invariants"
  "theorem"
  "end"
  "then"
  "variant"
  "events"
  "event"
  "any"
  "where"
  "when"
  "with"
  "witness"
  "begin"
  "status"
  "ordinary"
  "convergent"
  "anticipated"
] @keyword

[
  "∀"
  "∃"
  "·"
  "¬"
  "∧"
  "∨"
  "⇒"
  "⇔"
  "="
  "≠"
  "≤"
  "≥"
  "<"
  ">"
  "∈"
  "∉"
  "⊂"
  "⊄"
  "⊆"
  "⊈"
  "⦂"
  "↦"
  "↔"
  "<<->"
  "<->>"
  "<<->>"
  "⇸"
  "→"
  "⤔"
  "↣"
  "⤀"
  "↠"
  "⤖"
  "∪"
  "∩"
  "∖"
  "×"
  ";"
  "∘"
  "<+"
  "◁"
  "⩤"
  "▷"
  "⩥"
  "⊗"
  "∥"
  "‥"
  "+"
  "−"
  "∗"
  "÷"
  "mod"
  "^"
  "ℙ"
  "ℙ1"
  "dom"
  "ran"
  "∼"
  "λ"
  "⋃"
  "⋂"
  "∣"
  "≔"
  ":∈"
  ":∣"
] @operator

; A user-defined infix operator is a word; it is an operator nonetheless.
(infix_operator) @operator

; The closed operator words that are functions rather than relational
; operators; `dom`/`ran` sit with the operators above.
[
  "bool"
  "card"
  "max"
  "min"
] @function.builtin

[
  (true)
  (false)
  (bool_true)
  (bool_false)
  (integer_set)
  (natural_set)
  (natural1_set)
  (bool_set)
  (empty_set)
] @constant.builtin

; The number-set glyphs are tokens of their own (their ASCII spellings alias
; to them), so they are captured as tokens as well as through their nodes.
["ℤ" "ℕ" "ℕ1"] @constant.builtin

(builtin) @function.builtin

(function_application
  function: (identifier) @function)
(predicate_application
  function: (identifier) @function)
(function_override
  function: (identifier) @function)

(context name: (identifier) @module)
(machine name: (identifier) @module)
(refines_clause target: (identifier) @module)
(sees_clause (identifier) @module)
(extends_clause (identifier) @module)
(component_name (identifier) @module)
(set_declaration name: (identifier) @type)
(event name: (identifier) @function)
(event name: (component_name (identifier) @function))
(event refines: (identifier) @function)
(event refines: (component_name (identifier) @function))
(event extends: (identifier) @function)
(event extends: (component_name (identifier) @function))
(any_clause parameter: (identifier) @variable.parameter)
(typed_identifier name: (identifier) @variable.parameter)

; Rodin's mandatory initialisation event renders as a keyword, matching
; Rodin and Camille; like every keyword it is case-insensitive (rossi's
; kw_initialisation is ^"initialisation"). Character classes are the #match?
; subset shared by the Rust regex engine (CLI, Zed, Helix) and Neovim's
; vim-regex implementation, unlike (?i) or \c.
((event name: (identifier) @keyword)
  (#match? @keyword "^[Ii][Nn][Ii][Tt][Ii][Aa][Ll][Ii][Ss][Aa][Tt][Ii][Oo][Nn]$"))
