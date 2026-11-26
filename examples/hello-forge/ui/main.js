(function () {
  "use strict";

  const runtimeLabel = document.getElementById("runtime-label");
  const specRuntime = document.getElementById("spec-runtime");
  const specProcess = document.getElementById("spec-process");
  const specUa = document.getElementById("spec-ua");
  const greetForm = document.getElementById("greet-form");
  const nameInput = document.getElementById("name-input");
  const greetFeedback = document.getElementById("greet-feedback");
  const counterValue = document.getElementById("counter-value");
  const incOneBtn = document.getElementById("inc-one-btn");
  const incFiveBtn = document.getElementById("inc-five-btn");
  const clearLogBtn = document.getElementById("clear-log-btn");
  const ipcLog = document.getElementById("ipc-log");

  function recordLog(command, elapsedMs, summary, isError) {
    const empty = ipcLog.querySelector(".log-empty");
    if (empty) {
      empty.remove();
    }
    const item = document.createElement("li");
    if (isError) {
      item.className = "log-err";
    }
    item.textContent = `[${elapsedMs.toFixed(2)} ms] ${command}: ${summary}`;
    ipcLog.prepend(item);
  }

  async function timedInvoke(command, payload) {
    const t0 = performance.now();
    try {
      const result = await window.forge.invoke(command, payload);
      const dt = performance.now() - t0;
      recordLog(command, dt, JSON.stringify(result), false);
      return result;
    } catch (err) {
      const dt = performance.now() - t0;
      recordLog(command, dt, `${err.code}: ${err.message}`, true);
      throw err;
    }
  }

  async function loadHostStatus() {
    specUa.textContent = navigator.userAgent;
    try {
      const status = await timedInvoke("host.status");
      runtimeLabel.textContent = `Runtime: ${status.runtime}`;
      specRuntime.textContent = `${status.runtime} (window.forge.runtime = "${window.forge.runtime}")`;
      specProcess.textContent = `PID ${status.pid} on ${status.os}/${status.arch}`;
      counterValue.textContent = String(status.counter);
    } catch (err) {
      runtimeLabel.textContent = "Host connection error";
      greetFeedback.textContent = err.message;
      greetFeedback.classList.add("is-error");
    }
  }

  greetForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    nameInput.removeAttribute("aria-invalid");
    greetFeedback.classList.remove("is-error");

    try {
      const res = await timedInvoke("bench.greet", { name: nameInput.value });
      greetFeedback.textContent = res.greeting;
    } catch (err) {
      nameInput.setAttribute("aria-invalid", "true");
      greetFeedback.textContent = err.message;
      greetFeedback.classList.add("is-error");
    }
  });

  async function stepCounter(step) {
    try {
      const res = await timedInvoke("bench.increment", {
        step,
        sync_window_title: true,
      });
      counterValue.textContent = String(res.value);
    } catch (err) {
      greetFeedback.textContent = err.message;
      greetFeedback.classList.add("is-error");
    }
  }

  incOneBtn.addEventListener("click", () => stepCounter(1));
  incFiveBtn.addEventListener("click", () => stepCounter(5));

  clearLogBtn.addEventListener("click", () => {
    ipcLog.innerHTML = '<li class="log-empty">Invoke a command to inspect round-trip timing.</li>';
  });

  loadHostStatus();
})();
