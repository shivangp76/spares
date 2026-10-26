import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { useAuth } from '../hooks/useAuth';
import {
  CARD_ZOOMS, EDITOR_FONT_LABELS, EDITOR_FONT_SIZES, EDITOR_FONTS, LINE_WRAPPING_LABELS, LINE_WRAPPINGS, PAGE_WIDTH_LABELS,
  PAGE_WIDTHS, PAGES_PER_ROW, PAGES_PER_ROW_LABELS, setCardZoom, setCloseBrackets, setEditorFont, setEditorFontSize,
  setEditorLineNumbers, setEditorSpellcheck, setLineWrapping, setPageWidth, setPagesPerRow, setReadAloud,
  setShowReviewTimer, setVimKeybindings, useCardZoom, useCloseBrackets, useEditorFont, useEditorFontSize,
  useEditorLineNumbers, useEditorSpellcheck, useLineWrapping, usePageWidth, usePagesPerRow, useReadAloud,
  useShowReviewTimer, useVimKeybindings,
  type CardZoom, type EditorFontSize,
} from '../preferences';
import {
  setThemePreference, setTypstThemePreference, useThemePreference, useTypstThemePreference,
  type ThemePreference, type TypstThemePreference,
} from '../theme';
import { sectionLabel } from '../utils';

const THEMES: { value: ThemePreference; label: string }[] = [
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
  { value: 'system', label: 'System Preference' },
];

const TYPST_THEMES: { value: TypstThemePreference; label: string }[] = [
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
  { value: 'interface', label: 'Same as Interface' },
];

/** The voices this browser reads text aloud with, which some browsers list only after a while. */
function useSpeechVoices(): SpeechSynthesisVoice[] {
  const [voices, setVoices] = useState<SpeechSynthesisVoice[]>([]);
  useEffect(() => {
    if (typeof speechSynthesis === 'undefined') return;
    const update = () => setVoices(speechSynthesis.getVoices());
    update();
    speechSynthesis.addEventListener('voiceschanged', update);
    return () => speechSynthesis.removeEventListener('voiceschanged', update);
  }, []);
  return voices;
}

/** Settings kept in this browser. */
export default function SettingsPage() {
  const { credentials } = useAuth();
  const navigate = useNavigate();
  const theme = useThemePreference();
  const typstTheme = useTypstThemePreference();
  const showReviewTimer = useShowReviewTimer();
  const readAloud = useReadAloud();
  const voices = useSpeechVoices();
  const vimKeybindings = useVimKeybindings();
  const editorFontSize = useEditorFontSize();
  const editorFont = useEditorFont();
  const lineWrapping = useLineWrapping();
  const editorLineNumbers = useEditorLineNumbers();
  const editorSpellcheck = useEditorSpellcheck();
  const closeBrackets = useCloseBrackets();
  const pageWidth = usePageWidth();
  const cardZoom = useCardZoom();
  const pagesPerRow = usePagesPerRow();

  useEffect(() => {
    if (!credentials) navigate('/login');
  }, [credentials, navigate]);

  return (
    <div className="page">
      <h2 style={{ marginBottom: 16 }}>Settings</h2>

      <fieldset style={{ border: 'none', padding: 0, margin: 0 }}>
        <legend style={{ ...sectionLabel, padding: 0 }}>Theme</legend>
        <div style={{ display: 'flex', gap: 16, flexWrap: 'wrap' }}>
          {THEMES.map(({ value, label }) => (
            <label key={value} style={{ display: 'flex', gap: 6, alignItems: 'center', cursor: 'pointer' }}>
              <input
                type="radio"
                name="theme"
                value={value}
                checked={theme === value}
                onChange={() => setThemePreference(value)}
              />
              {label}
            </label>
          ))}
        </div>
      </fieldset>

      <fieldset style={{ border: 'none', padding: 0, margin: '24px 0 0' }}>
        <legend style={{ ...sectionLabel, padding: 0 }}>Layout</legend>
        <div style={{ display: 'flex', gap: 16, flexWrap: 'wrap', alignItems: 'center' }}>
          <span>Page width:</span>
          {PAGE_WIDTHS.map(width => (
            <label key={width} style={{ display: 'flex', gap: 6, alignItems: 'center', cursor: 'pointer' }}>
              <input
                type="radio"
                name="page-width"
                value={width}
                checked={pageWidth === width}
                onChange={() => setPageWidth(width)}
              />
              {PAGE_WIDTH_LABELS[width]}
            </label>
          ))}
        </div>
        <p style={{ fontSize: 12, color: 'var(--text-muted)', margin: '4px 0 0' }}>
          For large screens. Also changed with w while reviewing. Typst pages that fit the screen get longer lines, and pages of a fixed size get larger.
        </p>
        <label style={{ display: 'flex', gap: 6, alignItems: 'center', marginTop: 12 }}>
          Card text size:
          <select value={cardZoom} onChange={e => setCardZoom(e.target.value as CardZoom)}>
            {CARD_ZOOMS.map(zoom => <option key={zoom} value={zoom}>{zoom}%</option>)}
          </select>
        </label>
        <p style={{ fontSize: 12, color: 'var(--text-muted)', margin: '4px 0 0' }}>
          Wherever cards are shown. Also changed with + and - while reviewing. Typst pages that fit the screen are laid out narrower and scaled up; pages of a fixed size are unchanged.
        </p>
        <div style={{ display: 'flex', gap: 16, flexWrap: 'wrap', alignItems: 'center', marginTop: 12 }}>
          <span>Typst pages per row:</span>
          {PAGES_PER_ROW.map(pages => (
            <label key={pages} style={{ display: 'flex', gap: 6, alignItems: 'center', cursor: 'pointer' }}>
              <input
                type="radio"
                name="pages-per-row"
                value={pages}
                checked={pagesPerRow === pages}
                onChange={() => setPagesPerRow(pages)}
              />
              {PAGES_PER_ROW_LABELS[pages]}
            </label>
          ))}
        </div>
        <p style={{ fontSize: 12, color: 'var(--text-muted)', margin: '4px 0 0' }}>
          For cards of several pages, wherever they are shown. Two When Wide shows two side by side once the card is at least 1000 pixels wide, e.g. with the Wide page width. Always one on phones.
        </p>
      </fieldset>

      <fieldset style={{ border: 'none', padding: 0, margin: '24px 0 0' }}>
        <legend style={{ ...sectionLabel, padding: 0 }}>Typst Cards</legend>
        <div style={{ display: 'flex', gap: 16, flexWrap: 'wrap' }}>
          {TYPST_THEMES.map(({ value, label }) => (
            <label key={value} style={{ display: 'flex', gap: 6, alignItems: 'center', cursor: 'pointer' }}>
              <input
                type="radio"
                name="typst-theme"
                value={value}
                checked={typstTheme === value}
                onChange={() => setTypstThemePreference(value)}
              />
              {label}
            </label>
          ))}
        </div>
        <p style={{ fontSize: 12, color: 'var(--text-muted)', margin: '4px 0 0' }}>
          Also set by the Dark/Light button on each card. Dark inverts the page&apos;s colours, images included.
        </p>
      </fieldset>

      <fieldset style={{ border: 'none', padding: 0, margin: '24px 0 0' }}>
        <legend style={{ ...sectionLabel, padding: 0 }}>Review</legend>
        <label style={{ display: 'flex', gap: 6, alignItems: 'center', cursor: 'pointer' }}>
          <input
            type="checkbox"
            checked={showReviewTimer}
            onChange={e => setShowReviewTimer(e.target.checked)}
          />
          Show the running recall and rate timer
        </label>
        <p style={{ fontSize: 12, color: 'var(--text-muted)', margin: '4px 0 0' }}>
          Both durations are still recorded, and shown after each card is rated.
        </p>
        <label style={{ display: 'flex', gap: 6, alignItems: 'center', cursor: 'pointer', marginTop: 12 }}>
          <input
            type="checkbox"
            checked={readAloud}
            onChange={e => setReadAloud(e.target.checked)}
          />
          Read card text aloud
        </label>
        <p style={{ fontSize: 12, color: 'var(--text-muted)', margin: '4px 0 0' }}>
          For text marked with <code>tts</code>, read by this browser's voices. Audio clips play either way.
        </p>
        <details style={{ fontSize: 13, marginTop: 8 }}>
          <summary style={{ cursor: 'pointer' }}>Voices in this browser ({voices.length})</summary>
          {voices.length === 0 ? (
            <p style={{ color: 'var(--text-muted)', margin: '4px 0 0' }}>This browser lists no voices.</p>
          ) : (
            <>
              <p style={{ color: 'var(--text-muted)', margin: '4px 0' }}>
                Name one in a note, e.g. <code>tts:fr_FR,1,Thomas</code>, to read with it where it is available.
              </p>
              <ul style={{ margin: 0, paddingLeft: 20, maxHeight: 240, overflowY: 'auto' }}>
                {voices.map(voice => (
                  <li key={voice.voiceURI}>
                    {voice.name} <span style={{ color: 'var(--text-muted)' }}>({voice.lang}){voice.default ? ', default' : ''}</span>
                  </li>
                ))}
              </ul>
            </>
          )}
        </details>
      </fieldset>

      <fieldset style={{ border: 'none', padding: 0, margin: '24px 0 0' }}>
        <legend style={{ ...sectionLabel, padding: 0 }}>Editor</legend>
        <label style={{ display: 'flex', gap: 6, alignItems: 'center', cursor: 'pointer' }}>
          <input
            type="checkbox"
            checked={vimKeybindings}
            onChange={e => setVimKeybindings(e.target.checked)}
          />
          Vim keybindings in the note editor and typed answers, and j and k to move through lists
        </label>
        <p style={{ fontSize: 12, color: 'var(--text-muted)', margin: '4px 0 0' }}>
          Off by default on touchscreens, since a phone keyboard can't leave Vim's normal mode to type.
        </p>
        <label style={{ display: 'flex', gap: 6, alignItems: 'center', marginTop: 12 }}>
          Text size:
          <select value={editorFontSize} onChange={e => setEditorFontSize(e.target.value as EditorFontSize)}>
            {EDITOR_FONT_SIZES.map(size => <option key={size} value={size}>{size}px</option>)}
          </select>
        </label>
        <div style={{ display: 'flex', gap: 16, flexWrap: 'wrap', alignItems: 'center', marginTop: 12 }}>
          <span>Font:</span>
          {EDITOR_FONTS.map(font => (
            <label key={font} style={{ display: 'flex', gap: 6, alignItems: 'center', cursor: 'pointer' }}>
              <input
                type="radio"
                name="editor-font"
                value={font}
                checked={editorFont === font}
                onChange={() => setEditorFont(font)}
              />
              {EDITOR_FONT_LABELS[font]}
            </label>
          ))}
        </div>
        <p style={{ fontSize: 12, color: 'var(--text-muted)', margin: '4px 0 0' }}>
          For the note editor&apos;s text, tags and keywords. The typed answer box while reviewing is unchanged.
        </p>
        <div style={{ display: 'flex', gap: 16, flexWrap: 'wrap', alignItems: 'center', marginTop: 12 }}>
          <span>Wrap long lines:</span>
          {LINE_WRAPPINGS.map(wrapping => (
            <label key={wrapping} style={{ display: 'flex', gap: 6, alignItems: 'center', cursor: 'pointer' }}>
              <input
                type="radio"
                name="line-wrapping"
                value={wrapping}
                checked={lineWrapping === wrapping}
                onChange={() => setLineWrapping(wrapping)}
              />
              {LINE_WRAPPING_LABELS[wrapping]}
            </label>
          ))}
        </div>
        <p style={{ fontSize: 12, color: 'var(--text-muted)', margin: '4px 0 0' }}>
          Lines that aren&apos;t wrapped scroll sideways.
        </p>
        <label style={{ display: 'flex', gap: 6, alignItems: 'center', cursor: 'pointer', marginTop: 12 }}>
          <input
            type="checkbox"
            checked={editorLineNumbers}
            onChange={e => setEditorLineNumbers(e.target.checked)}
          />
          Line numbers beside the note&apos;s text
        </label>
        <p style={{ fontSize: 12, color: 'var(--text-muted)', margin: '4px 0 0' }}>
          Never on phones, to give the text the width.
        </p>
        <label style={{ display: 'flex', gap: 6, alignItems: 'center', cursor: 'pointer', marginTop: 12 }}>
          <input
            type="checkbox"
            checked={editorSpellcheck}
            onChange={e => setEditorSpellcheck(e.target.checked)}
          />
          Mark misspelled words in the note&apos;s text
        </label>
        <p style={{ fontSize: 12, color: 'var(--text-muted)', margin: '4px 0 0' }}>
          With the browser&apos;s spellchecker, in the languages it is set to check.
        </p>
        <label style={{ display: 'flex', gap: 6, alignItems: 'center', cursor: 'pointer', marginTop: 12 }}>
          <input
            type="checkbox"
            checked={closeBrackets}
            onChange={e => setCloseBrackets(e.target.checked)}
          />
          Type the closing bracket or quote along with the opening one
        </label>
      </fieldset>
    </div>
  );
}
