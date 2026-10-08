import React from "react";
import { createRoot } from "react-dom/client";

// Sets `data-window-shape` — must evaluate before any component module.
import "./lib/platform";
// Tab autosave/restore side effect (subscribes + reopens saved tabs).
import "./lib/tab-persist";
import App from "./App";
// Bundled fonts (offline Tauri): UI = Inter, code/numbers/logs = JetBrains Mono.
import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
import "@fontsource/jetbrains-mono/600.css";
import "./styles.css";

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
