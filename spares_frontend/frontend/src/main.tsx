import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import App from "./App.tsx";
import "./index.css";
import { credentialsKey, legacyCopyKey } from "./local/copyScope";
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

// Cookies and credentials are shared by tabs. Drop mounted editors when another tab
// changes sessions, before the old account can submit work into the new collection.
window.addEventListener("storage", (event) => {
  if (event.key === credentialsKey || event.key === null)
    window.location.reload();
  if (
    event.key === legacyCopyKey &&
    event.oldValue &&
    event.newValue &&
    JSON.parse(event.oldValue).collectionId !==
      JSON.parse(event.newValue).collectionId
  )
    window.location.reload();
});

// A frozen page may miss a storage event while another tab changes the login.
const initialCredentials = localStorage.getItem(credentialsKey);
window.addEventListener("pageshow", (event) => {
  if (
    event.persisted &&
    localStorage.getItem(credentialsKey) !== initialCredentials
  )
    window.location.reload();
});

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <BrowserRouter>
      <App />
    </BrowserRouter>
  </StrictMode>,
);
