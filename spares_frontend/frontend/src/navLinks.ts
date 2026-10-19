/** The pages the navbar links to, in its order, with the key that goes to each after `g`. */
export const NAV_LINKS = [
  { to: '/review', label: 'Review', key: 'r' },
  { to: '/notes', label: 'Notes', key: 'n' },
  { to: '/cards', label: 'Cards', key: 'c' },
  { to: '/tags', label: 'Tags', key: 't' },
  { to: '/parsers', label: 'Parsers', key: 'p' },
  { to: '/keywords', label: 'Keywords', key: 'k' },
  { to: '/image-occlusion', label: 'Image Occlusion', key: 'i' },
  { to: '/statistics', label: 'Statistics', key: 's' },
  // As Cmd+, opens an app's settings on a Mac
  { to: '/settings', label: 'Settings', key: ',' },
  { to: '/manual', label: 'Manual', key: 'm' },
];
