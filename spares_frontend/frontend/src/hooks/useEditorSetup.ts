import { useEffect, useMemo, useState } from 'react';
import { vim } from '@replit/codemirror-vim';
import { EditorView, type Extension } from '@uiw/react-codemirror';
import { imageUpload } from '../imageUpload';
import { clozeHighlighting, loadLanguage, type NoteSyntax, noteSyntax } from '../editorLanguages';
import {
  useCloseBrackets, useEditorFont, useEditorFontSize, useEditorLineNumbers, useEditorSpellcheck, useLineWrapping,
  useVimKeybindings,
} from '../preferences';
import { useMediaQuery } from './useMediaQuery';

/** What the note editors do beyond typing, for their Syntax popup. */
export const EDITOR_TIPS = ['Images pasted or dropped into the text are uploaded and inserted at that spot.'];

/** CodeMirror extensions and basic setups for the note editors, following the editor settings. On
    phones, line numbers are left out to give the text the width, and lines wrap unless turned
    off. `dataExtensions` and `dataSetup` are for the note's data, which also has the setting's line
    numbers and spellcheck, and highlights the syntax of the note's parser and its clozes;
    `extensions` and `fieldSetup` are for its tags and keywords. The parser's language is loaded on demand, so its highlighting appears
    once loaded. With `imageParserId`, images pasted or dropped into the data are uploaded and
    inserted in that parser's syntax. */
export function useEditorSetup(parserName?: string | null, imageParserId?: number | null) {
  const vimEnabled = useVimKeybindings();
  const fontSize = useEditorFontSize();
  const font = useEditorFont();
  const wrapping = useLineWrapping();
  const lineNumbers = useEditorLineNumbers();
  const spellcheck = useEditorSpellcheck();
  const closeBrackets = useCloseBrackets();
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

  const extensions = useMemo(() => [
    ...(vimEnabled ? [vim()] : []),
    ...(wrapping === 'always' || (wrapping === 'phones' && isNarrow) ? [EditorView.lineWrapping] : []),
    EditorView.theme({
      '&': { fontSize: `${fontSize}px` },
      // CodeMirror's own theme sets a monospace font here
      ...(font === 'interface' ? { '.cm-scroller': { fontFamily: 'inherit' } } : {}),
    }),
  ], [vimEnabled, isNarrow, fontSize, font, wrapping]);
  const dataExtensions = useMemo(() => {
    const images = imageParserId == null ? [] : [imageUpload(imageParserId)];
    const spelling = spellcheck ? [EditorView.contentAttributes.of({ spellcheck: 'true' })] : [];
    if (syntax === null) return [...extensions, ...spelling, ...images];
    // A language loaded for a previous parser is left out until this parser's has loaded
    const extension = language?.syntax === syntax ? [language.extension] : [];
    return [...extensions, ...spelling, ...extension, clozeHighlighting(syntax), ...images];
  }, [extensions, spellcheck, syntax, language, imageParserId]);
  const dataSetup = useMemo(() => ({ lineNumbers: lineNumbers && !isNarrow, closeBrackets }), [lineNumbers, isNarrow, closeBrackets]);
  const fieldSetup = useMemo(() => ({ lineNumbers: false, closeBrackets }), [closeBrackets]);
  return { extensions, dataExtensions, dataSetup, fieldSetup };
}
