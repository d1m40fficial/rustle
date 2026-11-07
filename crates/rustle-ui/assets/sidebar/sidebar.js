(() => {
  const send = (message) => {
    if (window.ipc) {
      window.ipc.postMessage(JSON.stringify(message));
    }
  };

  let current = {
    tabs: [],
    activeId: 0,
    theme: "dark",
    sidebarCollapsed: false,
    vertical: true,
  };

  const byId = (id) => document.getElementById(id);

  const initial = (tab) => {
    if (tab.error) return "!";
    if (tab.favicon) return "";
    const url = tab.url || "";
    if (!url || url.startsWith("rustle://")) return "R";
    try {
      return (new URL(url).hostname.replace(/^www\./, "")[0] || "R").toUpperCase();
    } catch {
      return "R";
    }
  };

  const hostFor = (tab) => {
    const url = tab.url || "";
    if (url.startsWith("rustle://")) {
      return url.replace("rustle://", "").split("/")[0].toUpperCase();
    }
    try {
      return new URL(url).hostname.replace(/^www\./, "");
    } catch {
      return "";
    }
  };

  const render = () => {
    document.documentElement.dataset.theme = current.theme === "light" ? "light" : "dark";
    document.body.classList.toggle("collapsed", Boolean(current.sidebarCollapsed));
    const count = byId("count");
    if (count) {
      count.textContent = String((current.tabs || []).length);
    }
    const collapse = byId("collapse");
    if (collapse) {
      collapse.textContent = current.sidebarCollapsed ? "›" : "‹";
      collapse.title = current.sidebarCollapsed ? "Развернуть панель" : "Свернуть панель";
    }
    const container = byId("tabs");
    container.replaceChildren();

    (current.tabs || []).forEach((tab) => {
      const row = document.createElement("div");
      row.className = "sidebar-tab" + (tab.id === current.activeId ? " sidebar-tab--active" : "");
      row.dataset.id = String(tab.id);
      row.title = tab.title || tab.url;

      const favicon = document.createElement("span");
      favicon.className = "sidebar-tab__favicon";
      if (tab.favicon) {
        const image = document.createElement("img");
        image.src = tab.faviconData || "rustle://localhost/favicon/" + tab.favicon;
        image.alt = "";
        favicon.append(image);
      } else {
        favicon.textContent = initial(tab);
      }

      const copy = document.createElement("span");
      copy.className = "sidebar-tab__copy";
      const title = document.createElement("span");
      title.className = "sidebar-tab__title";
      title.textContent = tab.title || "Вкладка";
      const host = document.createElement("span");
      host.className = "sidebar-tab__host";
      host.textContent = hostFor(tab);
      copy.append(title, host);

      const close = document.createElement("button");
      close.type = "button";
      close.className = "sidebar-tab__close";
      close.textContent = "×";
      close.title = "Закрыть вкладку";
      close.addEventListener("click", (event) => {
        event.stopPropagation();
        send({ type: "tabs.close", id: tab.id });
      });

      row.append(favicon, copy, close);
      row.addEventListener("click", () => send({ type: "tabs.activate", id: tab.id }));
      row.addEventListener("auxclick", (event) => {
        if (event.button === 1) {
          event.preventDefault();
          send({ type: "tabs.close", id: tab.id });
        }
      });
      container.append(row);
    });
  };

  window.rustle = {
    dispatch(message) {
      if (!message || message.type !== "state") return;
      current = { ...current, ...message.payload };
      render();
    },
  };

  byId("collapse").addEventListener("click", () => send({ type: "sidebar.collapse" }));
  byId("new-tab").addEventListener("click", () => send({ type: "tabs.new" }));
  send({ type: "sidebar.ready" });
})();