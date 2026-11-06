// browser.output(...) prints `##bmcp:result <json>` lines for the jobs runner.
browser.output({ email: inputs.email, step: 1 });
console.log("plain log line");
browser.output({ step: 2 });
