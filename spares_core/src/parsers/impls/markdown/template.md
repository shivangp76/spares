---
geometry: margin=1in
output: pdf_document
documentclass: extarticle
fontsize: 14pt
header-includes:
  - |
    ```{=latex}
    % A cloze in a table: soul's \hl (pandoc's [...]{.mark}) loops forever in a longtable with
    % pdflatex and some TeX Live versions (Ubuntu 26.04's), so a plain box there
    \AddToHook{env/longtable/begin}{\def\hl#1{\colorbox{yellow}{#1}}}
    ```
---
<!--- spares: body --->
