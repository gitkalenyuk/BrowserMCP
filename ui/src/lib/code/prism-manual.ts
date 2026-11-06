// Must run before prismjs is evaluated: keeps Prism from scanning the page on DOMContentLoaded
// and from installing a Web Worker message handler. We only call Prism.highlight ourselves.
(globalThis as { Prism?: unknown }).Prism = { manual: true, disableWorkerMessageHandler: true };

export {};
