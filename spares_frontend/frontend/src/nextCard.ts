// Getting the card a review is likely to show next ready while the current one is shown, so that
// rating shows it at once.
import { getQueuePositions, getReviewCardById } from "./api/client";
import { isPendingRender } from "./local/copy";
import { convertAhead } from "./local/pandoc";
import type { ReviewFilter } from "./types/spares";
import { lastReviewWidth, widthInputs } from "./typst/width";

/**
 * Asks for the card the review with `filter` would show next, other than `shownCardId` (the card
 * being reviewed, which stays first in the queue until it is rated), and prepares it: a Typst
 * card's sides are compiled at the width review cards were last shown at, and a Markdown card of a
 * note changed on this device is converted with pandoc, which also starts pandoc's worker.
 *
 * On a device that keeps a copy of the collection, the copy answers. Otherwise the server does,
 * and nothing is prepared when it can't be reached.
 */
export async function prepareNextCard(
  filter: ReviewFilter | null,
  shownCardId?: number,
): Promise<void> {
  const { positions } = await getQueuePositions({
    filter,
    match_query: null,
    limit: 2,
  });
  const next = positions.find((position) => position.card_id !== shownCardId);
  if (!next) return;
  const card = await getReviewCardById(next.card_id, filter ?? undefined);
  if (!card) return;
  if (card.device_sources && isPendingRender(card.note_id)) {
    convertAhead(card.device_sources.card_front);
    convertAhead(card.device_sources.card_back);
  }
  const width = lastReviewWidth();
  if (!card.browser_sources || width === null) return;
  const { precompileTypst } = await import("./typst/compiler");
  const inputs = widthInputs(width);
  await Promise.all([
    precompileTypst(card.browser_sources.card_front, inputs),
    precompileTypst(card.browser_sources.card_back, inputs),
  ]);
}
