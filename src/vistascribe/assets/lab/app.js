import { React, html } from "./lib/react.js";
import { createRoot } from "https://esm.sh/react-dom@18.3.1/client?dev&bundle";
import { LabApp } from "./components/LabApp.js";

const rootEl = document.getElementById("lab-root");
const root = createRoot(rootEl);
root.render(html`<${LabApp} />`);
