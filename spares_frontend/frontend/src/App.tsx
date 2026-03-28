// import { useEffect } from 'react'
import './App.css'
// import '/svgedit/src/editor/svgedit.css'
// import Editor from '/svgedit/src/editor/Editor.js'

function App() {
  // useEffect(() => {
  //   const svgEditor = new Editor(document.getElementById("container1"))
  //   // svgEditor.loadFromString('<svg width="500" height="500" xmlns="http://www.w3.org/2000/svg"><rect x="50" y="50" width="100" height="100" fill="blue" /></svg>');
  //   const extensions = [
  //     'ext-connector',
  //     'ext-eyedropper',
  //     'ext-grid',
  //     'ext-markers',
  //     'ext-panning',
  //     'ext-shapes',
  //     'ext-polystar',
  //     'ext-storage',
  //     'ext-opensave',
  //     'ext-layer_view'
  //   ]
  //   const otherExtensions = [
  //     // "ext-codemirror",
  //     // 'ext-helloworld',
  //     'ext-spares',
  //     // 'ext-xdomain-messaging',
  //   ]
  //   extensions.push(...otherExtensions);
  //
  //   // This is set in spares
  //   // const question_mask_fill_color = "FF7E7E"
  //   const other_mask_fill_color = "FFEBA2" // yellow
  //   const line_width = 1
  //   const line_color = "000000" // solid black
  //   const font_size = 16
  //   const font_family = "Sans-serif"
  //   svgEditor.setConfig({
  //     allowInitialUserOverride: true,
  //     noDefaultExtensions: true,
  //     no_save_warning: true,
  //     initFill: {
  //       color: other_mask_fill_color,
  //     },
  //     initStroke: {
  //       width: line_width,
  //       color: line_color,
  //     },
  //     text: {
  //       stroke_width: 0,
  //       font_size: font_size,
  //       font_family: font_family,
  //     },
  //     initTool: 'rect',
  //     // imgPath: "images/",
  //     imgPath: "/svgedit/src/editor/images/",
  //     allowedOrigins: ['null'],
  //     showlayers: true,
  //     // canvasName: "default",
  //     noStorageOnLoad: true,
  //     // dynamicOutput: false,
  //     // userExtensions: [/* { pathName: '/packages/react-test/dist/react-test.js' } */]
  //     extensions: extensions,
  //     // userExtensions: [ { pathName: '/packages/react-test/dist/react-test.js' } ]
  //   })
  //   svgEditor.init()
  // }, []);

  return (
    <>
      <h2>SVG Editor</h2>
      <div id="container1" style={{ width: "100%", height: "100vh", border: "1px solid black" }}></div>
    </>
  );
};

export default App
