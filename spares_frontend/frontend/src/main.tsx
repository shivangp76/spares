import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import App from "./App.tsx";
import "./index.css";
import { credentialsKey } from "./local/copyScope";
import { startLocalCopy } from "./local/copy";
import { startVersionChecks } from "./version";
import { onServerBack } from "./api/reachability";
import { usesLocalCopy } from "./local/transport";
import { dataChanged } from "./dataVersion";
import { appPath, DEMO } from "./demo";
import DemoLoading from "./components/DemoLoading";

// Lets the app open offline (see service-worker.js). Not in development, where files change, nor
// in the demo, which has nothing to update it from.
if (import.meta.env.PROD && !DEMO && "serviceWorker" in navigator) {
  navigator.serviceWorker.register(appPath("/sw.js")).catch(console.error);
}
// Notices when the server has a newer build than this page (see version.ts)
if (!DEMO) startVersionChecks();
// Opens this device's copy of the collection, if it keeps one, and syncs it. The demo opens it
// once it has loaded it (DemoLoading).
if (!DEMO) startLocalCopy().catch(console.error);
// Pages that failed while the server was unreachable fetch again once it is back. A device with a
// copy shows the copy, and its sync tells the pages when anything changed.
onServerBack(() => {
  if (!usesLocalCopy()) dataChanged();
});

// Cookies and credentials are shared by tabs. Drop mounted editors when another tab
// changes sessions, before the old account can submit work into the new collection.
window.addEventListener("storage", (event) => {
  if (event.key === credentialsKey || event.key === null)
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
    <BrowserRouter basename={appPath("") || "/"}>
      {DEMO ? (
        <DemoLoading>
          <App />
        </DemoLoading>
      ) : (
        <App />
      )}
    </BrowserRouter>
  </StrictMode>,
);
