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

  const openMenu = (event, kind, extra = {}) => {
    event.preventDefault();
    event.stopPropagation();
    send({
      type: "ui.menu",
      kind,
      origin: "sidebar",
      x: event.clientX,
      y: event.clientY,
      ...extra,
    });
  };

  const makeFavicon = (tab) => {
    const favicon = document.createElement("span");
    favicon.className = "sidebar-tab__favicon";
    if (tab.favicon) {
      const image = document.createElement("img");
      image.src = tab.faviconData || "rustle://localhost/favicon/" + tab.favicon;
      image.alt = "";
      image.addEventListener("error", () => {
        favicon.replaceChildren();
        favicon.textContent = initial(tab);
      });
      favicon.append(image);
    } else {
      favicon.textContent = initial(tab);
    }
    return favicon;
  };

  const makeRow = (tab) => {
    const row = document.createElement("div");
    row.className =
      "sidebar-tab" +
      (tab.pinned ? " sidebar-tab--pinned" : "") +
      (tab.id === current.activeId ? " sidebar-tab--active" : "");
    row.dataset.id = String(tab.id);
    row.title = tab.title || tab.url;

    const favicon = makeFavicon(tab);

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
    row.addEventListener("contextmenu", (event) => openMenu(event, "tab", { id: tab.id }));
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

    head.dataset.groupId = String(group.id);
    head.addEventListener("contextmenu", (event) =>
      openMenu(event, "group", { groupId: group.id }),
    );

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

    if (group.collapsed && tabs.length) {
      const favs = document.createElement("span");
      favs.className = "tab-group__favs";
      tabs.slice(0, 4).forEach((tab) => {
        const favicon = makeFavicon(tab);
        favicon.classList.add("tab-group__favicon");
        favs.append(favicon);
      });
      if (tabs.length > 4) {
        const more = document.createElement("span");
        more.className = "tab-group__more";
        more.textContent = "+" + (tabs.length - 4);
        favs.append(more);
      }
      head.append(toggle, name, count, favs, actions);
    } else {
      head.append(toggle, name, count, actions);
    }
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
    const pinned = tabs.filter((tab) => tab.pinned);
    const unpinned = tabs.filter((tab) => !tab.pinned);

    const pinnedContainer = byId("pinned");
    if (pinnedContainer) {
      pinnedContainer.replaceChildren();
      pinned.forEach((tab) => pinnedContainer.append(makeRow(tab)));
      pinnedContainer.hidden = pinned.length === 0;
    }

    const grouped = new Set();
    groups.forEach((group) => {
      const children = unpinned.filter((tab) => tab.groupId === group.id);
      children.forEach((tab) => grouped.add(tab.id));
      container.append(makeGroup(group, children));
    });
    unpinned
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
  send({ type: "sidebar.ready" });
})();