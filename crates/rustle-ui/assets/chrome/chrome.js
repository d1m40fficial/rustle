(() => {
  const send = (message) => {
    if (!window.ipc) {
      console.warn("IPC is not available");
      return;
    }
    window.ipc.postMessage(JSON.stringify(message));
  };

  let current = {
    url: "rustle://newtab",
    displayUrl: "rustle://newtab",
    title: "Новая вкладка",
    loading: false,
    canGoBack: false,
    canGoForward: false,
    error: null,
  };

  const byId = (id) => document.getElementById(id);

  const initialFor = (url) => {
    if (!url || url.startsWith("rustle://")) {
      return "R";
    }
    try {
      const host = new URL(url).hostname.replace(/^www\./, "");
      return (host[0] || "R").toUpperCase();
    } catch {
      return "R";
    }
  };

  const applyState = (payload) => {
    if (!payload || typeof payload !== "object") {
      return;
    }
    current = { ...current, ...payload };

    const title = payload.title || "Rustle";
    byId("tab-title").textContent = title;
    byId("tab").title = title;
    byId("tab-favicon").textContent = initialFor(payload.url);

    const address = byId("address");
    if (document.activeElement !== address) {
      address.value = payload.displayUrl || payload.url || "";
    }

    byId("back").disabled = !payload.canGoBack;
    byId("forward").disabled = !payload.canGoForward;

    const loading = Boolean(payload.loading);
    document.documentElement.classList.toggle("loading", loading);

    const reload = byId("reload");
    reload.dataset.cmd = loading ? "nav.stop" : payload.error ? "nav.retry" : "nav.reload";
    reload.title = loading ? "Остановить" : payload.error ? "Повторить" : "Обновить (F5)";
    reload.setAttribute("aria-label", reload.title);

    const error = payload.error;
    document.documentElement.classList.toggle("error-mode", Boolean(error));
    if (error) {
      byId("error-code").textContent = "Ошибка · " + (error.code || "unknown");
      byId("error-message").textContent = error.message || "Не удалось открыть страницу.";
      byId("error-url").textContent = payload.displayUrl || payload.url || "";
      byId("error-back").disabled = !payload.canGoBack;
    }
  };

  window.rustle = {
    dispatch(message) {
      if (!message || typeof message.type !== "string") {
        return;
      }
      if (message.type === "app.info") {
        const about = byId("about");
        if (about) {
          about.textContent = "M1 · " + message.version;
          about.title = "Rustle " + message.version + " (" + message.platform + ")";
        }
      } else if (message.type === "state") {
        applyState(message.payload);
      }
    },
  };

  const init = () => {
    const address = byId("address");

    address.addEventListener("focus", () => address.select());
    address.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        const url = address.value;
        address.blur();
        send({ type: "nav.go", url });
      } else if (event.key === "Escape") {
        address.value = current.displayUrl || current.url || "";
        address.blur();
      }
    });

    document.querySelectorAll("[data-cmd]").forEach((button) => {
      button.addEventListener("click", () => {
        send({ type: button.dataset.cmd });
      });
    });

    const go = byId("go");
    if (go) {
      go.addEventListener("click", () => {
        const url = address.value;
        address.blur();
        send({ type: "nav.go", url });
      });
    }

    const retry = byId("error-retry");
    if (retry) {
      retry.addEventListener("click", () => send({ type: "nav.retry" }));
    }

    const errorBack = byId("error-back");
    if (errorBack) {
      errorBack.addEventListener("click", () => send({ type: "nav.back" }));
    }

    const toggle = byId("overlay-toggle");
    if (toggle) {
      toggle.addEventListener("click", () => send({ type: "ui.toggleOverlay" }));
    }

    const close = byId("overlay-close");
    if (close) {
      close.addEventListener("click", () => send({ type: "ui.toggleOverlay" }));
    }

    send({ type: "chrome.ready" });
  };

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init, { once: true });
  } else {
    init();
  }
})();
