" Vim filetype detection file
" Language: Event-B
" Maintainer: Rossi Team
" Latest Revision: 2025-11-24

augroup eventb_ftdetect
  autocmd!

  " Detect .eventb files as Event-B filetype
  autocmd BufRead,BufNewFile *.eventb setfiletype eventb

  " Set some reasonable defaults for Event-B files
  autocmd FileType eventb setlocal commentstring=//\ %s
  autocmd FileType eventb setlocal comments=://,s1:/*,mb:*,ex:*/
  autocmd FileType eventb setlocal expandtab
  autocmd FileType eventb setlocal shiftwidth=4
  autocmd FileType eventb setlocal softtabstop=4
  autocmd FileType eventb setlocal tabstop=4
augroup END
