" Vim syntax file
" Language: Event-B
" Maintainer: Rossi Team
" Latest Revision: 2025-11-24

if exists("b:current_syntax")
  finish
endif

" Keywords
syn keyword eventbKeyword CONTEXT MACHINE END
syn keyword eventbKeyword EXTENDS SETS CONSTANTS AXIOMS THEOREMS
syn keyword eventbKeyword REFINES SEES VARIABLES INVARIANTS VARIANT EVENTS
syn keyword eventbKeyword INITIALISATION EVENT BEGIN THEN
syn keyword eventbKeyword ANY WHERE WHEN WITH WITNESS
syn keyword eventbKeyword STATUS ordinary convergent anticipated

" Constants
syn keyword eventbConstant TRUE FALSE BOOL INT NAT NAT1
syn keyword eventbConstant ⊤ ⊥

" Logical operators (Unicode)
syn match eventbOperator "¬\|∧\|∨\|⇒\|⇔"
syn match eventbOperator "∀\|∃"

" Logical operators (ASCII)
syn match eventbOperator "¬\|/\\\\\|\\\\/"
syn match eventbOperator "=>"
syn match eventbOperator "<=>"

" Set operators (Unicode)
syn match eventbOperator "∈\|∉\|⊂\|⊆\|⊄\|⊈"
syn match eventbOperator "∪\|∩\|∖\|×"
syn match eventbOperator "ℙ\|ℙ1"
syn match eventbOperator "∅"

" Set operators (ASCII)
syn match eventbOperator ":\|/:"
syn match eventbOperator "<<:\|<:\|/<:\|/<<:"
syn match eventbOperator "\\\\/\|/\\\\\|\\\\\\|\\*"
syn match eventbOperator "POW\|POW1"

" Relation operators (Unicode)
syn match eventbOperator "↔\|→\|⇸\|↣\|⤔\|↠\|⤀"
syn match eventbOperator "◁\|⩤\|▷\|⩥"
syn match eventbOperator "⊗\|∥"
syn match eventbOperator "↦"
syn match eventbOperator "∼\|⁻¹"
syn match eventbOperator "∘"
syn match eventbOperator "⊲\|⩥"

" Relation operators (ASCII)
syn match eventbOperator "<->\|-->\|+->\|>->\|>+>\|+->>\|>->>"
syn match eventbOperator "<|\|<<|\||>\||>>"
syn match eventbOperator ">\\*\|\\|\\|"
syn match eventbOperator "|->"
syn match eventbOperator "\\~"
syn match eventbOperator ";"
syn match eventbOperator "<+\|>-<"

" Arithmetic operators
syn match eventbOperator "=\|≠\|≤\|≥\|<\|>"
syn match eventbOperator "+\|-\|\\*\|/\|mod\|÷"
syn match eventbOperator "^\|\\.\\."
syn match eventbOperator "min\|max\|card"

" Special symbols
syn match eventbOperator "λ\|·\|∣\|⦂"
syn match eventbOperator "|"

" Brackets
syn match eventbDelimiter "{\|}\|(\|)\|\[\|\]"

" Numbers
syn match eventbNumber "\<\d\+\>"
syn match eventbNumber "-\d\+"

" Strings
syn region eventbString start='"' end='"' contains=eventbEscape
syn match eventbEscape "\\[nrt\\"]" contained

" Comments
syn match eventbComment "//.*$"
syn region eventbComment start="/\*" end="\*/"

" Identifiers (variables, constants, parameters, etc.)
syn match eventbIdentifier "\<[a-zA-Z_][a-zA-Z0-9_]*\>"

" Labels (for axioms, invariants, guards, actions, witnesses)
syn match eventbLabel "\<[a-zA-Z_][a-zA-Z0-9_]*\ze\s*:"

" Define the highlighting
hi def link eventbKeyword Keyword
hi def link eventbConstant Constant
hi def link eventbOperator Operator
hi def link eventbDelimiter Delimiter
hi def link eventbNumber Number
hi def link eventbString String
hi def link eventbEscape SpecialChar
hi def link eventbComment Comment
hi def link eventbIdentifier Identifier
hi def link eventbLabel Label

let b:current_syntax = "eventb"
