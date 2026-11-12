import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import App from "./App.tsx";
import "./index.css";
import { startLocalCopy } from "./local/copy";
import { startVersionChecks } from "./version";

// Lets the app open offline (see service-worker.js). Not in development, where files change.
if (import.meta.env.PROD && "serviceWorker" in navigator) {
  navigator.serviceWorker.register("/sw.js").catch(console.error);
}
// Notices when the server has a newer build than this page (see version.ts)
startVersionChecks();
// Opens this device's copy of the collection, if it keeps one, and syncs it
startLocalCopy().catch(console.error);

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <BrowserRouter>
      <App />
    </BrowserRouter>
  </StrictMode>,
);
