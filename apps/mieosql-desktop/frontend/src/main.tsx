import React from "react";
import { createRoot } from "react-dom/client";

// Sets `data-window-shape` — must evaluate before any component module.
import "./lib/platform";
// Tab autosave/restore side effect (subscribes + reopens saved tabs).
import "./lib/tab-persist";
import App from "./App";
import "./styles.css";

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
