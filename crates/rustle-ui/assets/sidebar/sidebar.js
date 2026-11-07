(() => {
  const send = (message) => {
    if (window.ipc) {
      window.ipc.postMessage(JSON.stringify(message));
    }
  };

  let current = {
    tabs: [],
    groups: [],
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

  const closeContextMenu = () => {
    byId("tab-context")?.remove();
  };

  const openContextMenu = (event, tab) => {
    event.preventDefault();
    event.stopPropagation();
    closeContextMenu();
    const menu = document.createElement("div");
    menu.className = "tab-context";
    menu.id = "tab-context";

    const addItem = (label, action, className = "") => {
      const item = document.createElement("button");
      item.type = "button";
      item.className = className;
      item.textContent = label;
      item.addEventListener("click", () => {
        closeContextMenu();
        action();
      });
      menu.append(item);
    };

    addItem("Новая группа…", () => {
      const name = window.prompt("Название группы", "Новая группа");
      if (name === null) return;
      send({ type: "groups.create", name: name.trim() || "Новая группа", tabId: tab.id });
    });

    (current.groups || []).forEach((group) => {
      if (group.id === tab.groupId) return;
      addItem("В группу «" + group.name + "»", () => {
        send({ type: "tabs.moveGroup", id: tab.id, groupId: group.id });
      });
    });

    if (tab.groupId) {
      addItem("Убрать из группы", () => {
        send({ type: "tabs.moveGroup", id: tab.id, groupId: null });
      });
    }

    document.body.append(menu);
    const rect = menu.getBoundingClientRect();
    const left = Math.min(event.clientX, window.innerWidth - rect.width - 8);
    const top = Math.min(event.clientY, window.innerHeight - rect.height - 8);
    menu.style.left = Math.max(6, left) + "px";
    menu.style.top = Math.max(6, top) + "px";
  };

  const makeRow = (tab) => {
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
    row.addEventListener("contextmenu", (event) => openContextMenu(event, tab));
    return row;
  };

  const makeGroup = (group, tabs) => {
    const section = document.createElement("section");
    section.className = "tab-group" + (group.collapsed ? " tab-group--collapsed" : "");
    const head = document.createElement("div");
    head.className = "tab-group__head";

    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.className = "tab-group__toggle";
    toggle.title = group.collapsed ? "Развернуть группу" : "Свернуть группу";
    toggle.textContent = group.collapsed ? "›" : "⌄";
    toggle.addEventListener("click", () => send({ type: "groups.toggle", groupId: group.id }));

    const name = document.createElement("span");
    name.className = "tab-group__name";
    name.textContent = group.name;
    name.title = group.name;

    const count = document.createElement("span");
    count.className = "tab-group__count";
    count.textContent = String(tabs.length);

    const actions = document.createElement("span");
    actions.className = "tab-group__actions";
    const add = document.createElement("button");
    add.type = "button";
    add.title = "Новая вкладка в группе";
    add.textContent = "+";
    add.addEventListener("click", (event) => {
      event.stopPropagation();
      send({ type: "groups.newTab", groupId: group.id });
    });
    const remove = document.createElement("button");
    remove.type = "button";
    remove.title = "Удалить группу";
    remove.textContent = "×";
    remove.addEventListener("click", (event) => {
      event.stopPropagation();
      if (window.confirm("Удалить группу «" + group.name + "» и закрыть её вкладки?")) {
        send({ type: "groups.remove", groupId: group.id });
      }
    });
    actions.append(add, remove);

    head.append(toggle, name, count, actions);
    section.append(head);

    if (!group.collapsed) {
      const body = document.createElement("div");
      body.className = "tab-group__tabs";
      tabs.forEach((tab) => body.append(makeRow(tab)));
      section.append(body);
    }
    return section;
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
    const groups = current.groups || [];
    const tabs = current.tabs || [];
    const grouped = new Set();
    groups.forEach((group) => {
      const children = tabs.filter((tab) => tab.groupId === group.id);
      children.forEach((tab) => grouped.add(tab.id));
      container.append(makeGroup(group, children));
    });
    tabs
      .filter((tab) => !grouped.has(tab.id))
      .forEach((tab) => container.append(makeRow(tab)));
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
  document.addEventListener("click", (event) => {
    if (!event.target.closest("#tab-context")) {
      closeContextMenu();
    }
  });
  document.addEventListener("contextmenu", (event) => {
    if (!event.target.closest(".sidebar-tab")) {
      closeContextMenu();
    }
  });
  send({ type: "sidebar.ready" });
})();
