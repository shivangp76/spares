# Image Occlusion Utility

Requirements:
- Ensure that multiple instances of the image occlusion editor can be run at once
- Should be a webpage so that multiple instances can easily be managed. Keep in mind that this is just a utility.
- The main UI for spares should be a separate binary. The main UI can also integrate an image occlusion editor but this smaller utility should remain for people who only use the CLI. Also, if they are combined, then you cannot run the main UI and this utility at the same time.

Workflow in the frontend:
- In a note's editor (Notes → New note, or a note's detail), click "Insert image occlusion". Or open the "Image Occlusion" page to get a block to paste into a note written elsewhere.
- Choose, paste or drop an image, then draw clozes on the Clozes layer and markup on the Markup layer. Add a cloze settings string to clozes, as needed.
- Click "Save and insert". The server stores the image and clozes in its image occlusion directory and returns the block in the note's parser syntax.
- To change an existing image occlusion, open its note and click "Edit image occlusion N". Each save stores a new pair of files and updates the note, so the change can be undone. Stored files are never overwritten since card orders are written into them.

The editor is SVG-Edit (the `svgedit` submodule, a fork with the `ext-spares` extension), embedded in an iframe with `?embedded=1`. The extension then hides its own open/save tools and exposes `window.sparesBridge` to the frontend.

Standalone workflow, for those who only use the CLI:
- Run `spares_frontend image-occlusion [--image <PATH>]` (or `spares_frontend io [--image <PATH>]`). The webpage should automatically open up with the image as its background, if provided, otherwise, click "Change Background Image" and choose an image. Each run is its own instance, on the next free port.
- Add markup and clozes to the appropriate layer. Add cloze settings string to clozes, as needed.
- Click "Save SVG".
- Navigate to note document and use a snippet to insert the image occlusion.

The editor's initial style is set by `[image_occlusion.editor]` in the spares config: `fill_color`, `stroke_color`, `stroke_width`, `font_size`, `font_family` and `initial_tool` (e.g. `rect`, `ellipse`, `fhpath` or `select`). The embedded editor gets them, along with the template, from `GET /api/image-occlusions/editor-config`, and the standalone editor gets them from `spares_frontend` when it starts. After changing them, reload the frontend or restart `spares_frontend`.

Press `?` in the editor, or choose "Keyboard Shortcuts" in its main menu, to list its keyboard shortcuts.

Potentially useful links:
- <https://github.com/SVG-Edit/svgedit>
- Method Draw (alternative to SVG-Edit): <https://github.com/methodofaction/Method-Draw/tree/master>
  - Addon using this Method Draw: <https://github.com/BlueGreenMagick/Image-Editor>
- SVG-Edit in Tauri: <https://github.com/brenoepics/svgedit-app>
