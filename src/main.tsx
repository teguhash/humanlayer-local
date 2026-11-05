import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles/globals.css";

// Follow the OS light/dark setting: Rosé Pine (dark) or Rosé Pine Dawn (light).
const media = window.matchMedia("(prefers-color-scheme: dark)");
function applyTheme() {
  const root = document.documentElement;
  root.dataset.theme = media.matches ? "rose-pine" : "rose-pine-dawn";
  root.classList.toggle("dark", media.matches);
}
applyTheme();
media.addEventListener("change", applyTheme);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
