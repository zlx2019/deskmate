// React 19 compatibility shim for animal-island-ui's imperative Notification.
// Its bundled React 18 react-dom/client CJS shim expects createRoot and
// hydrateRoot on the react-dom root export and the old __SECRET_INTERNALS name.
// Missing values cause Notification to throw during use. The library captures
// these references at module initialization, so main.tsx must import this file
// first. Remove it after the library gains React 19 support.

import ReactDOM from "react-dom";
import { createRoot, hydrateRoot } from "react-dom/client";

const rd = ReactDOM as unknown as Record<string, unknown>;
rd.__SECRET_INTERNALS_DO_NOT_USE_OR_YOU_WILL_BE_FIRED ??= {};
rd.createRoot ??= createRoot;
rd.hydrateRoot ??= hydrateRoot;
