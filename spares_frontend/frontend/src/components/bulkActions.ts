import { createTag, searchCardsPage } from '../api/client';
import type { BulkAction } from './BulkActionBar';

/** Makes a filtered tag of the cards a search finds, to review them, as Anki's filtered decks. */
export const FILTERED_TAG_ACTION: BulkAction = {
  label: 'Make filtered tag',
  wholeSearch: true,
  input: { kind: 'text', placeholder: 'Name of the new tag' },
  run: async (selector, [name]) => {
    if (!('Query' in selector)) throw new Error('A filtered tag is made from a search');
    const tag = await createTag({ name: name.trim(), description: '', query: selector.Query, auto_delete: false });
    const { total } = await searchCardsPage(`tag=${JSON.stringify(tag.name)}`, 1, 1);
    // The server doesn't return an event for it, so no Undo is offered
    return { message: `Made the filtered tag \`${tag.name}\`, with ${total} card(s). Review it from the Tags page.`, eventIds: [] };
  },
};
