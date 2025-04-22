# Image Occlusion Utility

Roadmap:
- Add any missing keyboard shortcuts. Most should already be present.
- Allow customizing initial fill color, along with other settings, specified in `svgedit/src/editor/index.html`
- Get the production build of the frontend to work with SVG-Edit. Currently only the dev build works.
- Programmatically enforce `spares/src/parsers/image_occlusion/template.svg` and the template manually included as a string in SVG-Edit are the same.
- Dynamically set the background. Allow the user to pass in the background image path to the CLI and that should open up the editor with the background preloaded.
  - Try `loadDataURI`: https://stackoverflow.com/questions/28450471/convert-inline-svg-to-base64-string
  - Local files cannot be linked to directly using `file:///...` because of Cross Site Scripting violations.

Requirements:
- Ensure that multiple instances of the image occlusion editor can be run at once
- Should be a webpage so that multiple instances can easily be managed. Keep in mind that this is just a utility.
- The main UI for spares should be a separate binary. The main UI can also integrate an image occlusion editor but this smaller utility should remain for people who only use the CLI. Also, if they are combined, then you cannot run the main UI and this utility at the same time.

Workflow:
- Run `spares_io` binary. The webpage should automatically open up.
- Click "Change Background Image" and choose an image.
- Add markup and clozes to the appropriate layer. Add cloze settings string to clozes, as needed.
- Click "Save SVG".
- Navigate to note document and use a snippet to insert the image occlusion.

Potentially useful links:
- <https://github.com/SVG-Edit/svgedit>
- Method Draw (alternative to SVG-Edit): <https://github.com/methodofaction/Method-Draw/tree/master>
  - Addon using this Method Draw: <https://github.com/BlueGreenMagick/Image-Editor>
- SVG-Edit in Tauri: <https://github.com/brenoepics/svgedit-app>
