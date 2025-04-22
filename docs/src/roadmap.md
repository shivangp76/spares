# Roadmap

- add documentation for filtered tags, image occlusion, front conceal, back reveal, etc.
- support grouped shapes in image occlusion

- find a way to integrate: <https://ankiweb.net/shared/info/969733775>
Standard setting: (previous visible, everything else hidden)
```md
- {{[g:1; g:3;hide:; g:4;hide:] a}}
- {{[g:1;hide:; g:2; g:4;hide:] b}}
- {{[g:1;hide:; g:2;hide:; g:3] c}}
- {{[g:1;hide:; g:2;hide:; g:3;hide:; g:4] d}}
```
Settings:
The defaults for these options should be able to be set in the config file.
- `context_before_each_item: u32`
    - This goes until there is 1 context before the first item or 1 context after the last item
- `prompts: u32` how many clozes should be required to answer?
- `context_after_each_item: u32`
- `no_cues_for_first_item: bool` default false. useful when you need to know the exact starting point and end point of a sequence
- `no_cues_for_last_item: bool` default false. useful when you need to know the exact starting point and end point of a sequence
- `start_and_end_gradually: bool` default false. ex. if `prompts = 4`, then it will show: 1 prompt, 2 prompt, 3 prompt, 4 prompt, 1 context 4 prompt, 2 context 4 prompt, etc. Instead of: 1 context 4 prompt, 2 context 4 prompt, etc.

- fix: linked notes parsing for typst. also parsing in general for typst.
- feat: add limit param to searching?
- TEST: Add initial migration code to add keywords and parser columns to Basic.
- TEST: Tag relations file when migrating
- fix: Better error handling for clozes. See `cloze_parser.rs`
- perf: Can parsing be made incremental instead of reading in the whole file?
- feat: Add undo capabilities
- feat: find a way to integrate: <https://ankiweb.net/shared/info/1491702369>
- feat: Add custom vim autocompletion menu for tags and linked notes. For example, if I press <C-t>, then a list of all tags matching the current word under the cursor show up. Another keybinding for keywords that I can link to for linked notes.
    - See [Obsidian Neovim Plugin](https://github.com/epwalsh/obsidian.nvim) as inspiration for Vim features.
- Add FSRS optimizer: <https://github.com/open-spaced-repetition/fsrs-rs>
    - `optimal_retention::simulate()`
    - `training.compute_parameters()`
    - Optimizing parameters improves performance by ~40% <https://github.com/open-spaced-repetition/go-fsrs/issues/19#issuecomment-2414421494>
    - <https://github.com/open-spaced-repetition/fsrs-browser>
- Improve code coverage
- Improve documentation coverage
- Fix unit test for when a card is converted to reverse only
- todos in code
- Implement some version of <https://github.com/Arthur-Milchior/anki-trigger-action-on-note?tab=readme-ov-file>. This is similar to <https://trane-project.github.io/faq.html#why-not-anki-or-another-existing-software> which suggests "defining arbitrary dependencies between subsets of flashcards and having the algorithm use those dependencies to select the flashcards to present is not supported". I can try to use the hierarchy provided by `tags` to somehow mimic this.
- Frontend: Tauri and React
    - ProseMirror for editor
    - <https://github.com/wojtekmaj/react-pdf> for displaying cards in the GUI
    - <http://www.material-react-table.com> for database explorer w/ UI
    - Backup since I can't do the same for LaTeX: <https://github.com/remarkjs/react-markdown>
    - Find some plug-and-play DB browser. Create a DB view to mirror Anki's layout and pass that into the DB browser. Ideally, customize the right click menu for each row so the user can edit that note/card.
    - Find some plug-and-play searching package that fuzzy searches across "data" and "keywords" fields
    - Backup: <https://tanstack.com/table/v8> for database explorer
    - Not possible: Use Dioxus or Leptos instead? I don't think I can use React libraries like prosemirror-react which I would need.
