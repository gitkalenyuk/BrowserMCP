// Idle in an await (like a browser command that never answers):
// V8 is not running, so the host's tokio timeout has to stop it.
console.log("waiting");
await sleep(60_000);
console.log("unreachable");
