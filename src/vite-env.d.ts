/// <reference types="vite/client" />

/**
 * True in the dev server and test runs, and in builds made with `DEMO_CLIENT=1`.
 * Release builds set it to false so the demo client is left out of the bundle.
 */
declare const __DEMO_CLIENT__: boolean;
