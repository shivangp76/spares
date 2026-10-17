import { useEffect, useState } from 'react';
import { getParserSyntax } from '../api/client';
import type { SyntaxExample } from '../types/spares';
import { errorMessage } from '../utils';

/** The examples of how notes are written for the parser, fetched once, or null while loading. */
export function useParserSyntax(parserId: number | null): { examples: SyntaxExample[] | null; error: string | null } {
  const [result, setResult] = useState<{ parserId: number; examples: SyntaxExample[] | null; error: string | null } | null>(null);
  useEffect(() => {
    if (parserId === null) return;
    let cancelled = false;
    getParserSyntax(parserId).then(
      examples => { if (!cancelled) setResult({ parserId, examples, error: null }); },
      e => { if (!cancelled) setResult({ parserId, examples: null, error: errorMessage(e) }); },
    );
    return () => { cancelled = true; };
  }, [parserId]);
  // A result for a previous parser is left out until this parser's has come
  return result?.parserId === parserId ? result : { examples: null, error: null };
}
