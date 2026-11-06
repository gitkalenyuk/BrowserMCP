// Never yields to the event loop: only V8 termination can stop it.
console.log("spinning");
let n: number = 0;
while (true) {
  n++;
}
