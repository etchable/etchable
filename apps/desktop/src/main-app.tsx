import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import ErrorBoundary, { GlobalErrorNotice } from "./ErrorBoundary";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {/* Last resort behind the per-panel boundaries: anything that escapes them
        still leaves a window you can read and reload, not a blank one. */}
    <ErrorBoundary what="The app">
      <App />
      {/* Async failures bypass the boundary entirely; this catches them. */}
      <GlobalErrorNotice />
    </ErrorBoundary>
  </React.StrictMode>,
);
