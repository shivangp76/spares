/** What a new, empty collection is offered on the review page (`components/Onboarding.tsx`). */

export type StarterParser = 'markdown' | 'typst' | 'latex-note';

export const STARTERS: { parser: StarterParser; label: string; first: string; sample: string }[] = [
  {
    parser: 'markdown',
    label: 'Markdown',
    first: 'The capital of France is {{ Paris }}.',
    sample: '# Capitals\n\nThe capital of France is {{Paris}}, and the capital of Japan is {{Tokyo}}.\n',
  },
  {
    parser: 'typst',
    label: 'Typst',
    first: 'The derivative of $x^2$ is #cl[$2x$].',
    sample: '= Derivatives\n\n$ d/(d x) x^n = #cl[$n x^(n-1)$] $\n\n$ d/(d x) e^x = #cl[$e^x$] $\n',
  },
  {
    parser: 'latex-note',
    label: 'LaTeX',
    first: 'The integral of $\\cos x$ is \\begin{cl}$\\sin x + C$\\end{cl}.',
    sample:
      '\\section*{Integration by parts}\n\n\\[ \\int u \\, dv = \\begin{cl}uv - \\int v \\, du\\end{cl} \\]\n',
  },
];

/** The tag sample notes have, so they are easy to find and delete. */
export const SAMPLE_TAG = 'sample';
