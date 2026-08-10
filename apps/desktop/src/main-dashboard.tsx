import React from "react";
import ReactDOM from "react-dom/client";
import Dashboard from "./Dashboard";
import ErrorBoundary, { GlobalErrorNotice } from "./ErrorBoundary";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary what="The dashboard">
      <Dashboard />
      {/* Async failures bypass the boundary entirely; this catches them. */}
      <GlobalErrorNotice />
    </ErrorBoundary>
  </React.StrictMode>,
);
