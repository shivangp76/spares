// The width a Typst source is shown at is passed to it as `sys.inputs.at("spares-width")`, in CSS
// pixels, so templates can lay out pages to fit the screen.
const WIDTH_INPUT = 'spares-width';
const REVIEW_WIDTH_KEY = 'spares_review_typst_width';

/** The inputs for a source shown `width` CSS pixels wide. */
export function widthInputs(width: number): Record<string, string> {
  return { [WIDTH_INPUT]: String(width) };
}

/** Remembers the width review cards were last compiled for, which the next ones likely are too. */
export function rememberReviewWidth(width: number): void {
  try {
    localStorage.setItem(REVIEW_WIDTH_KEY, String(width));
  } catch {
    // Without storage, cards are not compiled ahead of the review
  }
}

/** The width review cards were last compiled for, if known. */
export function lastReviewWidth(): number | null {
  try {
    const width = Number(localStorage.getItem(REVIEW_WIDTH_KEY));
    return width > 0 ? width : null;
  } catch {
    return null;
  }
}
