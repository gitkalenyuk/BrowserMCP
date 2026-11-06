// Prints what the host passed in `inputs` (for the --inputs-file test).
const big: string = inputs.big ?? "";
console.log(`name=${inputs.name} times=${inputs.times}`);
console.log(`big length=${big.length} nul=${big.includes(String.fromCharCode(0))}`);
