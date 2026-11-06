// A failing browser command becomes a catchable JS Error.
try {
  await browser.test.fail({ why: "testing" });
  console.log("not reached");
} catch (e) {
  console.log(`caught: ${(e as Error).message}`);
}
// The low-level form and the namespace form are the same call.
console.log(JSON.stringify(await browser.call("x.y", { a: 1 })));
// `await browser` must not hang: the proxies are not thenables.
console.log(typeof (await browser.tabs));
