(() => {
  "use strict";
  const TOKEN = __FORGE_TOKEN__;
  const RUNTIME = __FORGE_RUNTIME__;
  const WINDOW_LABEL = __FORGE_WINDOW__;
  const SMOKE = __FORGE_SMOKE__;
  const listeners = new Map();

  function dispatch(event, payload) {
    if (event === "__forge:css-reload") {
      const stamp = Date.now();
      document.querySelectorAll('link[rel="stylesheet"]').forEach((link) => {
        const href = link.getAttribute("href");
        if (href) {
          const clean = href.split("?")[0];
          link.setAttribute("href", clean + "?__forge_t=" + stamp);
        }
      });
      return;
    }
    if (event === "__forge:reload") {
      window.location.reload();
      return;
    }
    const set = listeners.get(event);
    if (!set) return;
    for (const fn of Array.from(set)) {
      try {
        fn(payload);
      } catch (err) {
        console.error("[FORGE] listener error on", event, err);
      }
    }
  }

  function listen(event, handler) {
    if (typeof event !== "string" || typeof handler !== "function") {
      throw new TypeError("forge.listen(event, handler) requires a string and a function");
    }
    let set = listeners.get(event);
    if (!set) {
      set = new Set();
      listeners.set(event, set);
    }
    set.add(handler);
    return () => {
      set.delete(handler);
      if (set.size === 0) listeners.delete(event);
    };
  }

  function once(event, handler) {
    const unlisten = listen(event, (payload) => {
      unlisten();
      handler(payload);
    });
    return unlisten;
  }

  async function invoke(command, payload) {
    if (typeof command !== "string" || command.length === 0) {
      const err = new Error("Command name must be a non-empty string");
      err.code = "FORGE_INVALID_COMMAND";
      throw err;
    }
    const res = await fetch("/__forge_ipc/" + encodeURIComponent(command), {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Forge-Token": TOKEN
      },
      body: JSON.stringify(payload === undefined ? null : payload)
    });
    const data = await res.json().catch(() => null);
    if (!res.ok) {
      const message = (data && data.message) || ("IPC failed with status " + res.status);
      const err = new Error(message);
      err.code = (data && data.code) || "APP_ERROR";
      if (data && data.details !== undefined) {
        err.details = data.details;
      }
      throw err;
    }
    return data;
  }

  function emit(event, payload) {
    return invoke("event.emit_all", { event, payload: payload === undefined ? null : payload });
  }

  function emitTo(target, event, payload) {
    return invoke("event.emit_to", {
      target,
      event,
      payload: payload === undefined ? null : payload
    });
  }

  const windowApi = Object.freeze({
    label: WINDOW_LABEL,
    close: () => invoke("window.close"),
    minimize: (minimized = true) => invoke("window.minimize", { minimized }),
    maximize: (maximized) => invoke("window.maximize", maximized === undefined ? null : { maximized }),
    toggleFullscreen: (fullscreen) =>
      invoke("window.toggle_fullscreen", fullscreen === undefined ? null : { fullscreen }),
    setTitle: (title) => invoke("window.set_title", { title }),
    show: () => invoke("window.show"),
    hide: () => invoke("window.hide"),
    focus: () => invoke("window.focus"),
    startDragging: () => invoke("window.start_dragging"),
    create: (opts) => invoke("window.create", opts),
    list: () => invoke("window.list")
  });

  Object.defineProperty(window, "__forge_dispatch", {
    value: dispatch,
    writable: false,
    configurable: false,
    enumerable: false
  });

  Object.defineProperty(window, "forge", {
    value: Object.freeze({
      runtime: RUNTIME,
      window: windowApi,
      invoke,
      listen,
      once,
      emit,
      emitTo
    }),
    writable: false,
    configurable: false,
    enumerable: true
  });

  window.addEventListener("DOMContentLoaded", () => {
    invoke("__forge_drain_events").then((queued) => {
      if (Array.isArray(queued)) {
        for (const [ev, payload] of queued) {
          dispatch(ev, payload);
        }
      }
      if (SMOKE) {
        return invoke("__forge_ready", { ua: navigator.userAgent });
      }
    }).catch((err) => {
      console.error("[FORGE] bootstrap drain/smoke error:", err);
    });
  }, { once: true });
})();
