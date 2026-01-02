import { React, html } from "./lib/react.js";
import { createRoot } from "react-dom/client";
import { LabApp } from "./components/LabApp.js";

const rootEl = document.getElementById("lab-root");
const root = createRoot(rootEl);
root.render(html`<${LabApp} />`);
