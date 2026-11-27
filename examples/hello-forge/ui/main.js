(function () {
  "use strict";

  const runtimeLabel = document.getElementById("runtime-label");
  const heartbeatLabel = document.getElementById("heartbeat-label");
  const specRuntime = document.getElementById("spec-runtime");
  const specProcess = document.getElementById("spec-process");
  const specUa = document.getElementById("spec-ua");
  const greetForm = document.getElementById("greet-form");
  const nameInput = document.getElementById("name-input");
  const greetFeedback = document.getElementById("greet-feedback");
  const counterValue = document.getElementById("counter-value");
  const incOneBtn = document.getElementById("inc-one-btn");
  const incFiveBtn = document.getElementById("inc-five-btn");
  const maxBtn = document.getElementById("max-btn");
  const restoreBtn = document.getElementById("restore-btn");
  const pinBtn = document.getElementById("pin-btn");
  const binEchoBtn = document.getElementById("bin-echo-btn");
  const closeBanner = document.getElementById("close-confirm-banner");
  const confirmDestroyBtn = document.getElementById("confirm-destroy-btn");
  const cancelCloseBtn = document.getElementById("cancel-close-btn");
  let pinnedTop = false;
  const noteInput = document.getElementById("note-input");
  const saveNoteBtn = document.getElementById("save-note-btn");
  const copyNoteBtn = document.getElementById("copy-note-btn");
  const noteFeedback = document.getElementById("note-feedback");
  const clearLogBtn = document.getElementById("clear-log-btn");
  const ipcLog = document.getElementById("ipc-log");

  function recordLog(prefix, summary, isError) {
    const empty = ipcLog.querySelector(".log-empty");
    if (empty) {
      empty.remove();
    }
    const item = document.createElement("li");
    if (isError) {
      item.className = "log-err";
    }
    item.textContent = `${prefix} ${summary}`;
    ipcLog.prepend(item);
  }

  async function timedInvoke(command, payload) {
    const t0 = performance.now();
    try {
      const result = await window.forge.invoke(command, payload);
      const dt = performance.now() - t0;
      recordLog(`[${dt.toFixed(2)} ms] ${command}:`, JSON.stringify(result), false);
      return result;
    } catch (err) {
      const dt = performance.now() - t0;
      recordLog(`[${dt.toFixed(2)} ms] ${command}:`, `${err.code}: ${err.message}`, true);
      throw err;
    }
  }

  // Subscribe to Rust -> JS background push events
  window.forge.listen("host.heartbeat", (payload) => {
    heartbeatLabel.textContent = `Heartbeat: #${payload.tick}`;
  });

  window.forge.listen("bench.greeted", (payload) => {
    recordLog("[event] bench.greeted:", payload.greeting, false);
  });

  window.forge.listen("window:close-requested", () => {
    closeBanner.hidden = false;
    recordLog("[event] window:close-requested:", "intercepted close (confirm or cancel)", false);
  });

  async function loadHostStatus() {
    specUa.textContent = navigator.userAgent;
    try {
      const status = await timedInvoke("host.status");
      runtimeLabel.textContent = `Runtime: ${status.runtime}`;
      specRuntime.textContent = `${status.runtime} (v${status.version})`;
      specProcess.textContent = `PID ${status.pid} on ${status.os}/${status.arch}`;
      counterValue.textContent = String(status.counter);

      // Load saved note if one exists in sandboxed storage
      const existing = await window.forge
        .invoke("fs.read_text", { path: "workbench-note.txt" })
        .catch(() => "");
      if (existing) {
        noteInput.value = existing;
      }
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
  maxBtn.addEventListener("click", () => timedInvoke("bench.window", { action: "maximize" }));
  restoreBtn.addEventListener("click", () => timedInvoke("bench.window", { action: "restore" }));
  pinBtn.addEventListener("click", async () => {
    pinnedTop = !pinnedTop;
    await window.forge.window.setAlwaysOnTop(pinnedTop);
    recordLog("[window] always_on_top:", String(pinnedTop), false);
  });
  binEchoBtn.addEventListener("click", async () => {
    const sample = new Uint8Array([0x01, 0x02, 0x03, 0xff]);
    const t0 = performance.now();
    const reversed = await window.forge.invokeBytes("bench.reverse_bytes", sample);
    const dt = performance.now() - t0;
    recordLog(`[${dt.toFixed(2)} ms] bench.reverse_bytes:`, Array.from(reversed).join(","), false);
  });
  confirmDestroyBtn.addEventListener("click", () => {
    window.forge.window.destroy();
  });
  cancelCloseBtn.addEventListener("click", () => {
    closeBanner.hidden = true;
  });

  saveNoteBtn.addEventListener("click", async () => {
    noteFeedback.classList.remove("is-error");
    try {
      await timedInvoke("fs.write_text", {
        path: "workbench-note.txt",
        contents: noteInput.value,
      });
      noteFeedback.textContent = "Saved note to sandboxed app data.";
    } catch (err) {
      noteFeedback.textContent = err.message;
      noteFeedback.classList.add("is-error");
    }
  });

  copyNoteBtn.addEventListener("click", async () => {
    noteFeedback.classList.remove("is-error");
    try {
      await timedInvoke("clipboard.write_text", { text: noteInput.value });
      noteFeedback.textContent = "Copied note to clipboard.";
    } catch (err) {
      noteFeedback.textContent = err.message;
      noteFeedback.classList.add("is-error");
    }
  });

  clearLogBtn.addEventListener("click", () => {
    ipcLog.innerHTML =
      '<li class="log-empty">Invoke a command or wait for a Rust push event.</li>';
  });

  loadHostStatus();
})();
