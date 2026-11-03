// A pending native operation only owns the dialog in which it started.
function createDialogTasks() {
  let epoch = 0;
  return {
    advance() { epoch += 1; },
    capture() { const owner = epoch; return () => owner === epoch; }
  };
}
