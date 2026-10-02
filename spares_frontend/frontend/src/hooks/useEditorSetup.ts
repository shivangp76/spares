import { useMemo } from 'react';
import { vim } from '@replit/codemirror-vim';
import { EditorView } from '@uiw/react-codemirror';
import { useVimKeybindings } from '../preferences';
import { useMediaQuery } from './useMediaQuery';

/** CodeMirror extensions and line numbers for the note editors, following the Vim setting. On
    phones, lines wrap instead of scrolling sideways, and line numbers are left out to give the
    text the width. */
export function useEditorSetup() {
  const vimEnabled = useVimKeybindings();
  const isNarrow = useMediaQuery('(max-width: 640px)');
  const extensions = useMemo(
    () => [...(vimEnabled ? [vim()] : []), ...(isNarrow ? [EditorView.lineWrapping] : [])],
    [vimEnabled, isNarrow],
  );
  return { extensions, lineNumbers: !isNarrow };
}
