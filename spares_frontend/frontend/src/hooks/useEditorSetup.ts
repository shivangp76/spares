import { useEffect, useMemo, useState } from 'react';
import { vim } from '@replit/codemirror-vim';
import { EditorView, type Extension } from '@uiw/react-codemirror';
import { clozeHighlighting, loadLanguage, type NoteSyntax, noteSyntax } from '../editorLanguages';
import { useVimKeybindings } from '../preferences';
import { useMediaQuery } from './useMediaQuery';

/** CodeMirror extensions and line numbers for the note editors, following the Vim setting. On
    phones, lines wrap instead of scrolling sideways, and line numbers are left out to give the
    text the width. `dataExtensions`, for the note's data, also highlight the syntax of the note's
    parser and its clozes. The parser's language is loaded on demand, so its highlighting appears
    once loaded. */
export function useEditorSetup(parserName?: string | null) {
  const vimEnabled = useVimKeybindings();
  const isNarrow = useMediaQuery('(max-width: 640px)');
  const syntax = noteSyntax(parserName);
  const [language, setLanguage] = useState<{ syntax: NoteSyntax; extension: Extension } | null>(null);

  useEffect(() => {
    if (syntax === null) return;
    let cancelled = false;
    loadLanguage(syntax).then(
      extension => { if (!cancelled) setLanguage({ syntax, extension }); },
      // Without the language, the note is still editable as plain text
      (e: unknown) => console.error(`Failed to load the ${syntax} editor language`, e),
    );
    return () => { cancelled = true; };
  }, [syntax]);

  const extensions = useMemo(
    () => [...(vimEnabled ? [vim()] : []), ...(isNarrow ? [EditorView.lineWrapping] : [])],
    [vimEnabled, isNarrow],
  );
  const dataExtensions = useMemo(() => {
    if (syntax === null) return extensions;
    // A language loaded for a previous parser is left out until this parser's has loaded
    const extension = language?.syntax === syntax ? [language.extension] : [];
    return [...extensions, ...extension, clozeHighlighting(syntax)];
  }, [extensions, syntax, language]);
  return { extensions, dataExtensions, lineNumbers: !isNarrow };
}
