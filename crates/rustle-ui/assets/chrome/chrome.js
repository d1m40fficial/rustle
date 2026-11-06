(() => {
  const send = (message) => {
    if (!window.ipc) {
      console.warn("IPC is not available");
      return;
    }
    window.ipc.postMessage(JSON.stringify(message));
  };

  let current = {
    tabs: [],
    activeId: 0,
    url: "rustle://newtab",
    displayUrl: "rustle://newtab",
    title: "Новая вкладка",
    loading: false,
    canGoBack: false,
    canGoForward: false,
    error: null,
  };

  const byId = (id) => document.getElementById(id);

  const initialFor = (tab) => {
    if (tab.error) {
      return "!";
    }
    const url = tab.url || "";
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

  const renderTabs = (tabs, activeId) => {
    const container = byId("tabs");
    container.replaceChildren();

    tabs.forEach((tab) => {
      const element = document.createElement("div");
      element.className =
        "tab" +
        (tab.id === activeId ? " tab--active" : "") +
        (tab.loading ? " tab--loading" : "");
      element.dataset.id = String(tab.id);
      element.setAttribute("role", "tab");
      element.setAttribute("aria-selected", String(tab.id === activeId));
      element.tabIndex = tab.id === activeId ? 0 : -1;
      element.title = tab.title || "Вкладка";

      const favicon = document.createElement("span");
      favicon.className = "tab__favicon";
      favicon.textContent = initialFor(tab);

      const spinner = document.createElement("span");
      spinner.className = "tab__spinner";
      spinner.setAttribute("aria-hidden", "true");

      const title = document.createElement("span");
      title.className = "tab__title";
      title.textContent = tab.title || "Вкладка";

      const close = document.createElement("button");
      close.className = "tab__close";
      close.type = "button";
      close.dataset.close = String(tab.id);
      close.title = "Закрыть вкладку";
      close.setAttribute("aria-label", "Закрыть вкладку");
      close.textContent = "×";

      element.append(favicon, spinner, title, close);
      container.append(element);
    });
  };

  const applyState = (payload) => {
    if (!payload || typeof payload !== "object") {
      return;
    }
    current = { ...current, ...payload };

    renderTabs(payload.tabs || [], payload.activeId || 0);

    const title = payload.title || "Rustle";
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

    document.title = title;
  };

  window.rustle = {
    dispatch(message) {
      if (!message || typeof message.type !== "string") {
        return;
      }
      if (message.type === "app.info") {
        const about = byId("about");
        if (about) {
          about.textContent = "M2 · " + message.version;
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

    const tabs = byId("tabs");
    tabs.addEventListener("click", (event) => {
      const close = event.target.closest("[data-close]");
      if (close) {
        event.stopPropagation();
        send({ type: "tabs.close", id: Number(close.dataset.close) });
        return;
      }
      const tab = event.target.closest(".tab");
      if (tab) {
        send({ type: "tabs.activate", id: Number(tab.dataset.id) });
      }
    });
    tabs.addEventListener("auxclick", (event) => {
      if (event.button !== 1) {
        return;
      }
      const tab = event.target.closest(".tab");
      if (tab) {
        event.preventDefault();
        send({ type: "tabs.close", id: Number(tab.dataset.id) });
      }
    });
    tabs.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        const tab = event.target.closest(".tab");
        if (tab) {
          event.preventDefault();
          send({ type: "tabs.activate", id: Number(tab.dataset.id) });
        }
      }
    });

    byId("new-tab").addEventListener("click", () => send({ type: "tabs.new" }));

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

    send({ type: "chrome.ready" });
  };

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init, { once: true });
  } else {
    init();
  }
})();
