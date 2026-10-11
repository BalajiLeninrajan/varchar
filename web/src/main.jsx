import posthog from "posthog-js";
import { render } from "preact";

import "catppuccin-neu/css/index.css";
import "./styles/app.css";
import { App } from "./App.jsx";

// Cookieless: nothing is written to cookies or storage. Panes that show the
// reader's SQL or data carry `ph-no-capture`, so autocapture skips them.
posthog.init(import.meta.env.VITE_PUBLIC_POSTHOG_KEY, {
  api_host: import.meta.env.VITE_PUBLIC_POSTHOG_HOST,
  defaults: "2026-08-30",
  cookieless_mode: "always",
});

render(<App />, document.getElementById("app"));
