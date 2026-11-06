// Fixture for tests/debugger.rs: line numbers below are used by the tests.
interface Item { n: number }
const items: Item[] = [{ n: 1 }, { n: 2 }, { n: 3 }];
let total = 0;
function add(x: number): number {
  const doubled = x * 2;
  return doubled;
}
for (const item of items) {
  total += add(item.n);
}
const tabs = await browser.tabs.list();
console.log("total", total, tabs.tabs.length);
