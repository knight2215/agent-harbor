import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./app";
// Import the theme store for its side effect: it resolves the persisted /
// system theme and applies `data-theme` on <html> before the first paint.
import "./state/theme";
import "./styles/tokens.css";
import "./styles/app.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
