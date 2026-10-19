import {
  GO_TO_SHORTCUTS,
  HELP_SHORTCUT,
  NOTE_EDITOR_SHORTCUTS,
  SEARCH_BOX_SHORTCUTS,
  type Shortcut,
} from './shortcuts';

/** A section of the manual, in `src/manual`. `pages.test.ts` checks that each one exists. */
export interface DocLink {
  label: string;
  /** The file in `src/manual`, like `concepts.md`. */
  file: string;
  /** The heading, as written in the file. */
  heading?: string;
}

/** What the help popup says about a page. */
export interface PageHelp {
  title: string;
  /** Paragraphs. Text in backticks is code, and fields in it are checked by `pages.test.ts`. */
  about: string[];
  shortcuts: Shortcut[];
  docs: DocLink[];
}

/** The pages of the manual, in the order it lists them. */
export const MANUAL_PAGES: { file: string; title: string }[] = [
  { file: 'getting_started.md', title: 'Getting Started' },
  { file: 'deploying.md', title: 'Deploying a Sync Server' },
  { file: 'concepts.md', title: 'Main Concepts' },
  { file: 'workflows.md', title: 'Workflows' },
  { file: 'comparison.md', title: 'Comparison with Anki' },
];

/** The anchor of a heading, as GitHub gives it, so the files' links work there too: lowercase,
    without punctuation, with `-` for spaces. */
export function headingAnchor(heading: string): string {
  return heading.trim().toLowerCase().replace(/[^\p{L}\p{N}\s_-]/gu, '').replace(/\s/g, '-');
}

/** The app's path of a page of the manual, as in `/manual/concepts`. */
export function manualPath(file: string): string {
  return `/manual/${file.replace(/\.md$/, '')}`;
}

/** Where a link in a page of the manual goes: the app's page for one to another file, as in
    `./workflows.md#jj`, and the link itself otherwise. */
export function manualHref(href: string): string {
  const match = /^(?:\.\/)?([\w-]+\.md)(#.*)?$/.exec(href);
  return match ? manualPath(match[1]) + (match[2] ?? '') : href;
}

export function docUrl(link: DocLink): string {
  return manualPath(link.file) + (link.heading ? `#${headingAnchor(link.heading)}` : '');
}

const NOTE_VS_CARD: DocLink = { label: 'Note vs Card', file: 'concepts.md', heading: 'Note vs Card' };
const SCHEDULING: DocLink = { label: 'Scheduling', file: 'concepts.md', heading: 'Scheduling' };

/** The help for each page, by its path. */
export const PAGE_HELP: Record<string, PageHelp> = {
  '/review': {
    title: 'Review',
    about: [
      'Reviews the cards due today, one at a time: recall what is hidden, show the answer, and rate how well you knew it. The rating decides when the card is next due.',
      'To review only some of them, enter a query, as on the Cards page, or pick a filtered tag.',
    ],
    // The review page lists its own, which apply only to some cards
    shortcuts: [],
    docs: [
      SCHEDULING,
      { label: 'Typed answers', file: 'getting_started.md', heading: 'Typed answers' },
      { label: 'Reviewing offline', file: 'getting_started.md', heading: 'Reviewing offline' },
    ],
  },
  '/notes': {
    title: 'Notes',
    about: [
      "A note holds what you want to remember. Each cloze in it, or group of clozes, makes a card: Syntax, by a note's text, shows how to write them.",
      'Tags group notes, as in `tag=spanish`. Keywords name what a note is about, as in `keyword="Irregular verbs"`, so that other notes can link to it.',
      'Check notes to act on them at once: add or remove tags, change their parser, export them to a file to import, or delete them. A search starts with every note it finds checked, on every page; uncheck the ones to leave out. Shift-click checks a range.',
      'Make filtered tag turns a search into a filtered tag, to review the cards it finds, as an Anki filtered deck does.',
    ],
    shortcuts: [...SEARCH_BOX_SHORTCUTS, ...NOTE_EDITOR_SHORTCUTS],
    docs: [
      NOTE_VS_CARD,
      { label: 'Clozes', file: 'concepts.md', heading: 'Clozes' },
      { label: 'Keywords', file: 'concepts.md', heading: 'Keywords' },
      { label: 'Linked notes', file: 'concepts.md', heading: 'Linked Notes' },
      { label: 'Editing notes', file: 'getting_started.md', heading: 'Editing notes' },
    ],
  },
  '/cards': {
    title: 'Cards',
    about: [
      "Each card asks for part of its note, and is scheduled on its own. Cards are searched for with the same queries as notes, and the card's own fields start with `c.`, as in `c.due<=today`.",
      'Check cards to suspend, bury, reschedule or forget them, or tag their notes, at once. A search starts with every card it finds checked, on every page; uncheck the ones to leave out. Shift-click checks a range.',
      'Make filtered tag turns a search into a filtered tag, to review the cards it finds, as an Anki filtered deck does.',
      'The bulk tools act on the cards a query finds, and list leeches.',
    ],
    shortcuts: [...SEARCH_BOX_SHORTCUTS],
    docs: [NOTE_VS_CARD, SCHEDULING],
  },
  '/tags': {
    title: 'Tags',
    about: [
      'Tags group notes and their cards. A tag named `a:b` is a child of `a`, and `tag=a` finds both.',
      'A tag with a query is a filtered tag, which holds the cards its query found when it was built. Rebuild finds them again, and Review studies them.',
    ],
    shortcuts: [...SEARCH_BOX_SHORTCUTS],
    docs: [{ label: 'Tags', file: 'concepts.md', heading: 'Tags' }],
  },
  '/parsers': {
    title: 'Parsers',
    about: [
      "A parser is the markup a note is written in, and decides how its clozes are written and how it is rendered. A parser listed here works only if spares has one by that name, in `spares_core::parsers::impls`.",
    ],
    shortcuts: [],
    docs: [
      { label: 'Parsers', file: 'concepts.md', heading: 'Parsers' },
      { label: 'Creating a parser', file: 'getting_started.md', heading: 'Creating a parser (optional)' },
    ],
  },
  '/keywords': {
    title: 'Keywords',
    about: [
      'Keywords name what a note is about. A note links to another by one of its keywords, which is found even when written a little differently, as Search shows.',
    ],
    shortcuts: [],
    docs: [
      { label: 'Keywords', file: 'concepts.md', heading: 'Keywords' },
      { label: 'Linked notes', file: 'concepts.md', heading: 'Linked Notes' },
    ],
  },
  '/image-occlusion': {
    title: 'Image Occlusion',
    about: [
      'Draw clozes over an image, each of which makes a card, and paste the block into a note written elsewhere. In the note editor, "Insert image occlusion" does this in place.',
    ],
    shortcuts: [],
    docs: [{ label: 'Image occlusions', file: 'getting_started.md', heading: 'Image Occlusions' }],
  },
  '/statistics': {
    title: 'Statistics',
    about: ['How your reviews went, and how many cards are due in the days ahead. A query limits them to the cards it finds.'],
    shortcuts: [...SEARCH_BOX_SHORTCUTS],
    docs: [SCHEDULING],
  },
  '/settings': {
    title: 'Settings',
    about: [
      "These settings are kept in this browser. Scheduling, the daily limits and the presets are set in the server's `config.toml`.",
    ],
    shortcuts: [],
    docs: [SCHEDULING, { label: 'Deploying a sync server', file: 'deploying.md' }],
  },
};

/** Linked from the login page. */
export const LOGIN_DOCS: DocLink = { label: 'Setting up a server to log in to', file: 'deploying.md', heading: '5. Log in on the phone' };

/** The shortcuts every page has, after the page's own. */
export const GLOBAL_SHORTCUTS: Shortcut[] = [HELP_SHORTCUT, ...GO_TO_SHORTCUTS];
