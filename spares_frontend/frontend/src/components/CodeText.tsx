/** Text from the server whose parts in backticks are code, like `c.due<=today`. */
export default function CodeText({ text }: { text: string }) {
  // Splitting on a captured group keeps the code at the odd indices
  return <>{text.split(/`([^`]*)`/).map((part, i) => (i % 2 === 1 ? <code key={i}>{part}</code> : part))}</>;
}
