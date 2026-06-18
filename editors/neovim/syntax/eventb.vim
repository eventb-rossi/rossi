" Vim syntax file
" Language: Event-B
" Maintainer: Rossi Team
" Latest Revision: 2025-11-24

if exists("b:current_syntax")
  finish
endif

" >>> rossi gen-grammars (generated, do not edit)
syn case ignore
syn keyword eventbKeyword any axioms begin constants context end event events extends initialisation invariants machine refines sees sets status then theorems variables variant when where with witness
syn keyword eventbStatusKeyword anticipated convergent ordinary skip theorem
syn case match
syn keyword eventbConstant BOOL FALSE INT NAT NAT1 TRUE bool false true
syn keyword eventbBuiltin card finite id max min partition pred prj1 prj2 succ
syn keyword eventbOperator INTER POW POW1 UNION circ dom mod not oftype or ran

syn match eventbConstant "ℕ1\|ℕ\|ℤ\|∅\|{}"
syn match eventbOperator "<<->>\|/<<:\|:∈\|:∣\|<->>\|<<->\|>->>\|ℙ1\|+->\|+>>\|-->\|->>\|/<:\|<->\|<<:\|<<|\|<=>\|>+>\|>->\||->\||>>\|‥\|ℙ\|→\|↔\|↠\|↣\|↦\|⇒\|⇔\|⇸\|∀\|∃\|∈\|∉\|−\|∖\|∗\|∘\|∣\|∥\|∧\|∨\|∩\|∪\|∼\|≔\|≠\|≤\|≥\|⊂\|⊄\|⊆\|⊈\|⊗\|⋂\|⋃\|▷\|◁\|⤀\|⤔\|⤖\|⦂\|⩤\|⩥\|\|\|\|\|\*\*\|\.\.\|/:\|/=\|/\\\|::\|:=\|:|\|<+\|<:\|<=\|<|\|=>\|><\|>=\|\\/\||>\|||\|¬\|·\|×\|÷\|λ\|!\|#\|%\|&\|\*\|+\|-\|\.\|/\|:\|;\|<\|=\|>\|\\\|\^\||\|\~"

syn match eventbNumber "\<\d\+\>"
syn region eventbString start='"' end='"' contains=eventbEscape
syn match eventbEscape "\\[nrt\\\"]" contained
syn match eventbComment "//.*$"
syn region eventbComment start="/\*" end="\*/"
syn match eventbLabel "@[A-Za-z0-9_]\+"
syn match eventbIdentifier "\<[a-zA-Z_][a-zA-Z0-9_]*\>"
syn match eventbDelimiter "[(){}\[\]]"

hi def link eventbKeyword Keyword
hi def link eventbStatusKeyword Keyword
hi def link eventbConstant Constant
hi def link eventbBuiltin Function
hi def link eventbOperator Operator
hi def link eventbNumber Number
hi def link eventbString String
hi def link eventbEscape SpecialChar
hi def link eventbComment Comment
hi def link eventbLabel Label
hi def link eventbIdentifier Identifier
hi def link eventbDelimiter Delimiter
" <<< rossi gen-grammars

let b:current_syntax = "eventb"
