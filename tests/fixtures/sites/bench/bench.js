// Shared by the benchmark fixtures (tests/bench, `cargo xtask bench`).
// `benchSubmit(task, payload)` reports what the agent did to the fixture server
// (POST /bench/result); the runner checks it after the agent finishes. The page only says
// "Submitted" — whether the answer is right is decided by the runner, not shown here.
window.benchSubmit = async function (task, payload) {
  let status = document.getElementById('bench-status');
  if (!status) {
    status = document.createElement('p');
    status.id = 'bench-status';
    status.setAttribute('role', 'status');
    document.body.append(status);
  }
  try {
    const res = await fetch('/bench/result', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ task, payload }),
    });
    status.textContent = res.ok ? 'Submitted.' : `Submit failed (HTTP ${res.status}).`;
  } catch (err) {
    status.textContent = `Submit failed: ${err}`;
  }
};
