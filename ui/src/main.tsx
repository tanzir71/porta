import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { desktopPlatform } from "./lib/platform";
import "./styles.css";

document.documentElement.dataset.platform = desktopPlatform;

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
