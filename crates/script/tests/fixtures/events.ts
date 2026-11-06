// Run by tests/events.rs: handlers, patterns, unsubscribe and a throwing handler.
let seen = 0;
const off = browser.on("tab.*", async (data: { id: number }, topic: string) => {
  seen++;
  await browser.call("test.seen" as never, { topic, id: data.id } as never);
  if (seen === 2) off(); // later tab.* events must not arrive here any more
});

// A failing handler is reported on stderr and must not stop the script.
browser.on("tab.updated", () => {
  throw new Error("boom");
});

browser.on("never.*", () => browser.call("test.never" as never, {} as never));
