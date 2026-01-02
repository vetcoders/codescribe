// Use import map-provided React to ensure a single instance across the app.
import React from "react";
import htm from "https://esm.sh/htm@3.1.1?pin=v135";

export { React };
export const html = htm.bind(React.createElement);
