import { spawn } from "node:child_process";
import { constants } from "node:os";

const child = spawn("/bin/bash", [process.argv[2]], {
  detached: true,
  stdio: "inherit",
});
let exited = false;
let exitCode = 1;
let interrupted = 0;
let stopping = false;
let escalated = false;

function signalGroup(signal) {
  try {
    process.kill(-child.pid, signal);
    return true;
  } catch (error) {
    if (error.code === "ESRCH") return false;
    throw error;
  }
}

function finish() {
  if (exited && (!stopping || escalated)) process.exit(interrupted || exitCode);
}

function stop() {
  if (stopping) return;
  stopping = true;
  signalGroup("SIGTERM");
  //The fixture owns this process group, including pipe-holding descendants of an exited shell.
  setTimeout(() => {
    signalGroup("SIGKILL");
    escalated = true;
    finish();
  }, 250);
}

for (const signal of ["SIGINT", "SIGHUP", "SIGTERM"]) {
  process.on(signal, () => {
    interrupted = 128 + constants.signals[signal];
    stop();
  });
}

child.once("error", (error) => {
  console.error(error.message);
  process.exit(1);
});
child.once("exit", (code, signal) => {
  exited = true;
  exitCode = code ?? 128 + constants.signals[signal];
  if (!stopping && signalGroup(0)) stop();
  finish();
});
