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
    mode: "newtab",
    internalPage: "",
    theme: "dark",
    groups: [],
    adblockStatus: null,
    settings: { searchEngine: "ddg", theme: "system", startup: "restore" },
  };

  let currentInternalPage = "";
  let nextRequestId = 1;
  const pending = new Map();

  const byId = (id) => document.getElementById(id);

  const request = (type, payload = {}) =>
    new Promise((resolve, reject) => {
      const id = nextRequestId++;
      pending.set(id, { resolve, reject });
      send({ ...payload, type, id });
    });

  let lastZoomPercent = null;
  let zoomHideTimer = 0;
  const showZoomIndicator = () => {
    const label = byId("zoom-label");
    if (!label) {
      return;
    }
    label.classList.add("is-visible");
    window.clearTimeout(zoomHideTimer);
    zoomHideTimer = window.setTimeout(() => label.classList.remove("is-visible"), 2200);
  };

  let suggestionItems = [];
  let suggestionIndex = -1;

  const closeSuggestions = () => {
    const panel = byId("suggestions");
    if (!panel) {
      return;
    }
    if (!panel.hidden) {
      panel.hidden = true;
      send({ type: "ui.dropdown", open: false });
    }
    suggestionItems = [];
    suggestionIndex = -1;
  };

  const chooseSuggestion = (item) => {
    if (!item) {
      return;
    }
    byId("address").value = item.url;
    closeSuggestions();
    send({ type: "nav.go", url: item.url });
  };

  const renderSuggestions = (items) => {
    const panel = byId("suggestions");
    if (!panel) {
      return;
    }
    suggestionItems = items;
    suggestionIndex = items.length ? 0 : -1;
    panel.replaceChildren();
    items.forEach((item, index) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "suggestion" + (index === suggestionIndex ? " is-selected" : "");
      const title = document.createElement("span");
      title.className = "suggestion__title";
      title.textContent = item.title || item.url;
      const url = document.createElement("span");
      url.className = "suggestion__url";
      url.textContent = item.url;
      button.append(title, url);
      button.addEventListener("mousedown", (event) => event.preventDefault());
      button.addEventListener("click", () => chooseSuggestion(item));
      panel.append(button);
    });
    if (!items.length) {
      panel.hidden = true;
      send({ type: "ui.dropdown", open: false });
      return;
    }
    panel.hidden = false;
    const height = Math.min(360, 78 + 8 + items.length * 52);
    send({ type: "ui.dropdown", open: true, height });
  };

  const selectSuggestion = (delta) => {
    if (!suggestionItems.length) {
      return;
    }
    suggestionIndex =
      (suggestionIndex + delta + suggestionItems.length) % suggestionItems.length;
    const panel = byId("suggestions");
    [...panel.querySelectorAll(".suggestion")].forEach((button, index) => {
      button.classList.toggle("is-selected", index === suggestionIndex);
    });
  };

  const formatTime = (seconds) => {
    if (!seconds) {
      return "";
    }
    try {
      return new Intl.DateTimeFormat("ru-RU", {
        dateStyle: "short",
        timeStyle: "short",
      }).format(new Date(seconds * 1000));
    } catch {
      return "";
    }
  };

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

  const makeTabFavicon = (tab) => {
    const favicon = document.createElement("span");
    favicon.className = "tab__favicon";
    if (tab.favicon) {
      favicon.classList.add("tab__favicon--image");
      const image = document.createElement("img");
      image.src = tab.faviconData || "rustle://localhost/favicon/" + tab.favicon;
      image.alt = "";
      image.addEventListener("error", () => {
        favicon.classList.remove("tab__favicon--image");
        favicon.replaceChildren();
        favicon.textContent = initialFor(tab);
      });
      favicon.append(image);
    } else {
      favicon.textContent = initialFor(tab);
    }
    return favicon;
  };

  const groupHue = (group) => Number(group.hue ?? ((Number(group.id) * 53) % 360));

  const makeGroupChip = (group, children) => {
    const chip = document.createElement("div");
    chip.className = "tab-group__chip";
    chip.dataset.groupToggle = String(group.id);
    chip.dataset.groupId = String(group.id);
    chip.tabIndex = 0;
    chip.setAttribute("role", "button");
    chip.setAttribute("aria-expanded", String(!group.collapsed));
    chip.title = group.name;
    chip.style.setProperty("--group-hue", groupHue(group));

    const name = document.createElement("span");
    name.className = "tab-group__name";
    name.textContent = group.name;
    chip.append(name);
    return chip;
  };

  const makeTabElement = (tab, activeId, pinned) => {
    const element = document.createElement("div");
    element.className =
      "tab" +
      (pinned ? " tab--pinned" : "") +
      (tab.id === activeId ? " tab--active" : "") +
      (tab.loading ? " tab--loading" : "");
    element.dataset.id = String(tab.id);
    element.draggable = true;
    element.setAttribute("role", "tab");
    element.setAttribute("aria-selected", String(tab.id === activeId));
    element.tabIndex = tab.id === activeId ? 0 : -1;
    element.title = tab.title || "Вкладка";

    const favicon = makeTabFavicon(tab);

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
    element.addEventListener("dragstart", (event) => {
      event.dataTransfer.effectAllowed = "move";
      event.dataTransfer.setData("text/plain", String(tab.id));
      element.classList.add("tab--dragging");
    });
    element.addEventListener("dragover", (event) => {
      event.preventDefault();
      event.dataTransfer.dropEffect = "move";
      element.classList.add("tab--drop");
    });
    element.addEventListener("dragleave", () => element.classList.remove("tab--drop"));
    element.addEventListener("drop", (event) => {
      event.preventDefault();
      element.classList.remove("tab--drop");
      const target = Number(event.dataTransfer.getData("text/plain"));
      if (target && target !== tab.id) {
        send({ type: "tabs.move", id: target, target: tab.id });
      }
    });
    element.addEventListener("dragend", () => {
      element.classList.remove("tab--dragging");
      document.querySelectorAll(".tab--drop").forEach((node) => node.classList.remove("tab--drop"));
    });
    return element;
  };

  const makeGroupElement = (group, children, activeId) => {
    const section = document.createElement("div");
    section.className =
      "tab-group tab-group--horizontal" + (group.collapsed ? " tab-group--collapsed" : "");
    section.dataset.groupId = String(group.id);
    section.style.setProperty("--group-hue", groupHue(group));
    section.append(makeGroupChip(group, children));
    if (!group.collapsed) {
      const body = document.createElement("div");
      body.className = "tab-group__tabs";
        children.forEach((tab) => body.append(makeTabElement(tab, activeId, false)));
      section.append(body);
    }
    return section;
  };

  const renderTabs = (tabs, activeId) => {
    const container = byId("tabs");
    container.replaceChildren();

    const pinned = tabs.filter((tab) => tab.pinned);
    const rest = tabs.filter((tab) => !tab.pinned);
    pinned.forEach((tab) => container.append(makeTabElement(tab, activeId, true)));
    if (pinned.length) {
      const divider = document.createElement("span");
      divider.className = "tabs__divider";
      container.append(divider);
    }

    const groups = current.groups || [];
    const renderedGroups = new Set();
    rest.forEach((tab) => {
      const groupId = tab.groupId;
      if (groupId === null || groupId === undefined) {
        container.append(makeTabElement(tab, activeId, false));
        return;
      }
      if (renderedGroups.has(groupId)) {
        return;
      }
      const group = groups.find((entry) => entry.id === groupId);
      if (!group) {
        container.append(makeTabElement(tab, activeId, false));
        return;
      }
      renderedGroups.add(groupId);
      const children = rest.filter((entry) => entry.groupId === groupId);
      container.append(makeGroupElement(group, children, activeId));
    });

    const active = container.querySelector(".tab--active");
    if (active) {
      active.scrollIntoView({ inline: "nearest", block: "nearest" });
    }
  };

  const emptyRow = (text) => {
    const element = document.createElement("div");
    element.className = "page-empty";
    element.textContent = text;
    return element;
  };

  const listRow = (title, url, time, onOpen, onDelete = null) => {
    const row = document.createElement("div");
    row.className = "page-row";

    const main = document.createElement("button");
    main.className = "page-row__main";
    main.type = "button";
    const titleNode = document.createElement("span");
    titleNode.className = "page-row__title";
    titleNode.textContent = title || url;
    const urlNode = document.createElement("span");
    urlNode.className = "page-row__url";
    urlNode.textContent = url;
    main.append(titleNode, urlNode);
    main.addEventListener("click", onOpen);
    row.append(main);

    const timeNode = document.createElement("span");
    timeNode.className = "page-row__time";
    timeNode.textContent = time || "";
    row.append(timeNode);

    if (onDelete) {
      const remove = document.createElement("button");
      remove.className = "page-row__remove";
      remove.type = "button";
      remove.title = "Удалить";
      remove.setAttribute("aria-label", "Удалить");
      remove.textContent = "×";
      remove.addEventListener("click", onDelete);
      row.append(remove);
    } else {
      row.append(document.createElement("span"));
    }

    return row;
  };

  const loadHistory = async (query = "") => {
    const list = byId("history-list");
    list.replaceChildren(emptyRow("Загрузка…"));
    try {
      const data = await request("history.query", { query, limit: 300 });
      list.replaceChildren();
      if (!data.ok) {
        throw new Error(data.error || "storage error");
      }
      const items = data.items || [];
      if (!items.length) {
        list.append(emptyRow(query ? "Ничего не найдено." : "История пуста."));
        return;
      }
      items.forEach((item) => {
        list.append(
          listRow(
            item.title || item.url,
            item.url,
            formatTime(item.visited_at),
            () => send({ type: "nav.go", url: item.url }),
            async () => {
              await request("history.delete", { entryId: item.id });
              loadHistory(byId("history-search").value.trim());
            },
          ),
        );
      });
    } catch (error) {
      list.replaceChildren(emptyRow("Не удалось загрузить историю."));
      console.warn(error);
    }
  };

  const loadBookmarks = async () => {
    const list = byId("bookmarks-list");
    list.replaceChildren(emptyRow("Загрузка…"));
    try {
      const data = await request("bookmarks.list");
      list.replaceChildren();
      if (!data.ok) {
        throw new Error(data.error || "storage error");
      }
      const items = data.items || [];
      if (!items.length) {
        list.append(emptyRow("Закладок пока нет."));
        return;
      }
      const groups = new Map();
      items.forEach((item) => {
        const folder = item.folder || "";
        if (!groups.has(folder)) {
          groups.set(folder, []);
        }
        groups.get(folder).push(item);
      });
      groups.forEach((group, folder) => {
        if (folder) {
          const heading = document.createElement("div");
          heading.className = "bookmark-folder";
          heading.textContent = folder;
          list.append(heading);
        }
        group.forEach((item) => {
          list.append(
            listRow(
              item.title || item.url,
              item.url,
              formatTime(item.created_at),
              () => send({ type: "nav.go", url: item.url }),
              async () => {
                await request("bookmarks.remove", { entryId: item.id });
                loadBookmarks();
                refreshBookmarkTiles();
              },
            ),
          );
        });
      });
    } catch (error) {
      list.replaceChildren(emptyRow("Не удалось загрузить закладки."));
      console.warn(error);
    }
  };

  const DEFAULT_SHORTCUTS = [
    { url: "https://github.com", title: "GitHub", mark: "GH" },
    { url: "https://www.youtube.com", title: "YouTube", mark: "YT" },
    { url: "https://mail.google.com", title: "Gmail", mark: "GM" },
  ];

  const shortcutsKey = "rustle.shortcuts.v1";
  const tileOrderKey = "rustle.tiles.order.v1";
  const tileLabelsKey = "rustle.tiles.labels.v1";
  let tileEdit = false;
  const tileFavicons = new Map();
  const tileFaviconRequested = new Set();
  let tileFaviconPending = false;

  const readShortcuts = () => {
    try {
      const raw = localStorage.getItem(shortcutsKey);
      if (raw !== null) {
        const parsed = JSON.parse(raw);
        if (Array.isArray(parsed)) {
          return parsed;
        }
      }
      localStorage.setItem(shortcutsKey, JSON.stringify(DEFAULT_SHORTCUTS));
    } catch {
      // ignore malformed storage
    }
    return DEFAULT_SHORTCUTS;
  };

  const saveShortcuts = (items) => {
    try {
      localStorage.setItem(shortcutsKey, JSON.stringify(items));
    } catch {
      // localStorage may be disabled; tiles still work for this session
    }
  };

  const readTileOrder = () => {
    try {
      const parsed = JSON.parse(localStorage.getItem(tileOrderKey) || "[]");
      return Array.isArray(parsed) ? parsed : [];
    } catch {
      return [];
    }
  };

  const saveTileOrder = (urls) => {
    try {
      localStorage.setItem(tileOrderKey, JSON.stringify(urls));
    } catch {
      // ignore
    }
  };

  const readTileLabels = () => {
    try {
      const parsed = JSON.parse(localStorage.getItem(tileLabelsKey) || "{}");
      return parsed && typeof parsed === "object" ? parsed : {};
    } catch {
      return {};
    }
  };

  const saveTileLabels = (labels) => {
    try {
      localStorage.setItem(tileLabelsKey, JSON.stringify(labels));
    } catch {
      // ignore
    }
  };

  const hostForUrl = (url, fallback = "") => {
    try {
      return new URL(url).hostname.replace(/^www\./, "") || fallback;
    } catch {
      return fallback || url;
    }
  };

  const requestTileFavicons = (items) => {
    if (tileFaviconPending) {
      return;
    }
    const urls = [];
    items.forEach((item) => {
      const url = item.url || "";
      if (!/^https?:\/\//i.test(url)) {
        return;
      }
      if (tileFavicons.has(url) || tileFaviconRequested.has(url)) {
        return;
      }
      tileFaviconRequested.add(url);
      urls.push(url);
    });
    if (!urls.length) {
      return;
    }
    tileFaviconPending = true;
    request("favicon.query", { urls: urls.slice(0, 96) })
      .then((data) => {
        const found = (data && data.items) || {};
        Object.keys(found).forEach((url) => tileFavicons.set(url, found[url]));
      })
      .catch((error) => console.warn(error))
      .finally(() => {
        tileFaviconPending = false;
        renderShortcuts();
      });
  };

  const renderShortcuts = () => {
    const container = byId("newtab-shortcuts");
    if (!container) {
      return;
    }
    const custom = readShortcuts();
    const customUrls = new Set(custom.map((item) => item.url));
    const labels = readTileLabels();
    const bookmarks = (current.bookmarks || [])
      .filter((item) => item.url && !customUrls.has(item.url))
      .map((item) => {
        const host = hostForUrl(item.url);
        const title = (item.title || "").trim() || host;
        return {
          url: item.url,
          title,
          bookmark: true,
        };
      });
    const items = [...custom, ...bookmarks].map((item) => {
      let title = (item.title || "").trim();
      if (!title || /^https?:\/\//.test(title) || title === item.url) {
        title = hostForUrl(item.url, title || item.url);
      }
      if (labels[item.url]) {
        title = labels[item.url];
      }
      return { ...item, title };
    });
    const order = readTileOrder();
    if (order.length) {
      items.sort((a, b) => {
        const ai = order.indexOf(a.url);
        const bi = order.indexOf(b.url);
        if (ai < 0 && bi < 0) {
          return 0;
        }
        if (ai < 0) {
          return 1;
        }
        if (bi < 0) {
          return -1;
        }
        return ai - bi;
      });
    }

    container.classList.toggle("is-editing", tileEdit);
    container.replaceChildren();
    items.forEach((item) => {
      const tile = document.createElement("div");
      tile.className = "newtab__shortcut" + (tileEdit ? " shortcut--editable" : "");
      tile.tabIndex = 0;
      tile.setAttribute("role", "button");
      tile.dataset.url = item.url;
      tile.title = tileEdit ? "Перетащите плитку или нажмите карандаш" : item.title;
      const mark = document.createElement("span");
      mark.className = "newtab__mark";
      const faviconData = tileFavicons.get(item.url);
      if (faviconData) {
        mark.classList.add("newtab__mark--image");
        const image = document.createElement("img");
        image.src = faviconData;
        image.alt = "";
        image.addEventListener("error", () => {
          mark.classList.remove("newtab__mark--image");
          mark.replaceChildren();
          mark.textContent = item.title.slice(0, 2).toUpperCase();
        });
        mark.append(image);
      } else {
        mark.textContent = item.title.slice(0, 2).toUpperCase();
      }
      const label = document.createElement("span");
      label.className = "shortcut__label";
      label.textContent = item.title;
      const rename = document.createElement("button");
      rename.type = "button";
      rename.className = "shortcut__rename";
      rename.title = "Переименовать плитку";
      rename.setAttribute("aria-label", "Переименовать плитку");
      rename.textContent = "✎";
      rename.addEventListener("click", (event) => {
        event.stopPropagation();
        const next = window.prompt("Название плитки", item.title);
        if (next === null) {
          return;
        }
        const trimmed = next.trim();
        if (!trimmed) {
          return;
        }
        const map = readTileLabels();
        map[item.url] = trimmed;
        saveTileLabels(map);
        renderShortcuts();
      });
      tile.append(mark, label, rename);
      if (tileEdit) {
  const remove = document.createElement("button");
  remove.type = "button";
  remove.className = "shortcut__remove";
  remove.title = "Удалить плитку";
  remove.setAttribute("aria-label", "Удалить плитку");
  remove.textContent = "×";
  remove.addEventListener("click", async (event) => {
    event.stopPropagation();
    if (item.bookmark) {
      try {
        await request("bookmarks.toggle", { url: item.url, title: item.title });
        current.bookmarks = (current.bookmarks || []).filter(
          (entry) => entry.url !== item.url,
        );
      } catch (error) {
        console.warn(error);
      }
    } else {
      const next = readShortcuts().filter((entry) => entry.url !== item.url);
      saveShortcuts(next);
    }
    const order = readTileOrder().filter((url) => url !== item.url);
    saveTileOrder(order);
    const labels = readTileLabels();
    delete labels[item.url];
    saveTileLabels(labels);
    renderShortcuts();
  });
  tile.append(remove);
}

      if (tileEdit) {
        tile.draggable = true;
        tile.addEventListener("dragstart", (event) => {
          event.dataTransfer.effectAllowed = "move";
          event.dataTransfer.setData("text/plain", item.url);
          tile.classList.add("shortcut--dragging");
        });
        tile.addEventListener("dragover", (event) => {
          event.preventDefault();
          event.dataTransfer.dropEffect = "move";
          tile.classList.add("shortcut--drop");
        });
        tile.addEventListener("dragleave", () => tile.classList.remove("shortcut--drop"));
        tile.addEventListener("drop", (event) => {
          event.preventDefault();
          tile.classList.remove("shortcut--drop");
          const sourceUrl = event.dataTransfer.getData("text/plain");
          if (!sourceUrl || sourceUrl === item.url) {
            return;
          }
          const urls = items.map((entry) => entry.url);
          const from = urls.indexOf(sourceUrl);
          const to = urls.indexOf(item.url);
          if (from < 0 || to < 0) {
            return;
          }
          const [moved] = urls.splice(from, 1);
          urls.splice(to, 0, moved);
          saveTileOrder(urls);
          renderShortcuts();
        });
        tile.addEventListener("dragend", () => {
          container
            .querySelectorAll(".shortcut--dragging, .shortcut--drop")
            .forEach((node) => node.classList.remove("shortcut--dragging", "shortcut--drop"));
        });
      }

      tile.addEventListener("click", () => {
        if (!tileEdit) {
          send({ type: "nav.go", url: item.url });
        }
      });
      tile.addEventListener("keydown", (event) => {
        if (tileEdit) {
          return;
        }
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          send({ type: "nav.go", url: item.url });
        }
      });
      container.append(tile);
    });
    requestTileFavicons(items);
  };

  const formatBytes = (bytes) => {
    if (!bytes) {
      return "";
    }
    const units = ["Б", "КБ", "МБ", "ГБ"];
    let value = bytes;
    let index = 0;
    while (value >= 1024 && index < units.length - 1) {
      value /= 1024;
      index += 1;
    }
    return value.toFixed(index ? 1 : 0) + " " + units[index];
  };

  const renderSessions = (items = current.sessions || []) => {
    const list = byId("sessions-list");
    if (!list) return;
    list.replaceChildren();
    if (!items.length) {
      const empty = document.createElement("span");
      empty.className = "page-hint";
      empty.textContent = "Сохранённых сессий пока нет.";
      list.append(empty);
      return;
    }
    items.forEach((item) => {
      const card = document.createElement("div");
      card.className = "session-card";
      const title = document.createElement("strong");
      title.textContent = item.name || "Сессия";
      const meta = document.createElement("small");
      meta.textContent = (item.count || 0) + " вкладок · " + (item.preview || []).slice(0, 2).join(" / ");
      const actions = document.createElement("div");
      actions.className = "session-card__actions";
      const open = document.createElement("button");
      open.className = "button";
      open.type = "button";
      open.textContent = "Открыть";
      open.addEventListener("click", () => send({ type: "sessions.load", id: item.id }));
      const remove = document.createElement("button");
      remove.className = "button";
      remove.type = "button";
      remove.textContent = "Удалить";
      remove.addEventListener("click", () => send({ type: "sessions.delete", id: item.id }));
      actions.append(open, remove);
      card.append(title, meta, actions);
      list.append(card);
    });
  };

  const renderDownloads = (items = current.downloads || []) => {
    const list = byId("downloads-list");
    if (!list) {
      return;
    }
    list.replaceChildren();
    if (!items.length) {
      list.append(emptyRow("Загрузок пока нет."));
      return;
    }
    items.forEach((item) => {
      const row = document.createElement("div");
      row.className = "download-row";
      const main = document.createElement("div");
      main.className = "download-row__main";
      const title = document.createElement("span");
      title.className = "page-row__title";
      const name = (item.path || "").split(/[\\/]/).pop() || item.url;
      title.textContent = name;
      const url = document.createElement("span");
      url.className = "page-row__url";
      url.textContent = item.url;
      main.append(title, url);

      const info = document.createElement("div");
      info.className = "download-row__info";
      const percent =
        item.total > 0 ? Math.min(100, Math.round((item.received / item.total) * 100)) : 0;
      const bar = document.createElement("div");
      bar.className = "download-bar";
      const fill = document.createElement("span");
      fill.style.width = percent + "%";
      bar.append(fill);
      const status = document.createElement("span");
      status.className = "download-row__status";
      const labels = {
        progress: item.total > 0 ? percent + "%" : formatBytes(item.received),
        completed: "Готово",
        interrupted: "Прервано",
        cancelled: "Отменено",
      };
      status.textContent = labels[item.state] || item.state;
      info.append(bar, status);

      const actions = document.createElement("div");
      actions.className = "download-row__actions";
      if (item.state === "progress") {
        const cancel = document.createElement("button");
        cancel.className = "button";
        cancel.type = "button";
        cancel.textContent = "Отменить";
        cancel.addEventListener("click", () => send({ type: "downloads.cancel", id: item.id }));
        actions.append(cancel);
      } else {
        const open = document.createElement("button");
        open.className = "button";
        open.type = "button";
        open.textContent = "Открыть";
        open.addEventListener("click", () => send({ type: "downloads.open", id: item.id }));
        const reveal = document.createElement("button");
        reveal.className = "button";
        reveal.type = "button";
        reveal.textContent = "Папка";
        reveal.addEventListener("click", () => send({ type: "downloads.reveal", id: item.id }));
        actions.append(open, reveal);
      }

      row.append(main, info, actions);
      list.append(row);
    });
  };

  const adblockStatusText = () => {
    const info = current.adblockStatus || {};
    if (info.loading) {
      return info.message || "Обновление фильтров…";
    }
    if (info.ready) {
      const lists = Array.isArray(info.lists) && info.lists.length ? " · " + info.lists.join(", ") : "";
      const updated = info.updatedAt ? " · " + formatTime(info.updatedAt) : "";
      return (info.message || "Фильтры готовы") + lists + updated;
    }
    return info.message || "Движок ещё не готов";
  };

  const updateAdblockStatus = () => {
    const status = byId("adblock-status");
    if (status) {
      status.textContent = adblockStatusText();
    }
    const button = byId("adblock-update");
    if (button) {
      button.disabled = Boolean(current.adblockStatus && current.adblockStatus.loading);
      button.textContent = current.adblockStatus && current.adblockStatus.loading ? "Обновление…" : "Обновить";
    }
  };

  const populateSettings = () => {
    const settings = current.settings || {};
    const engine = byId("setting-engine");
    const theme = byId("setting-theme");
    const startup = byId("setting-startup");
    if (engine) {
      engine.value = settings.searchEngine || "ddg";
    }
    if (theme) {
      theme.value = settings.theme || "system";
    }
    if (startup) {
      startup.value = settings.startup || "restore";
    }
    const sidebarWidth = byId("setting-sidebar-width");
    if (sidebarWidth) {
      sidebarWidth.value = String(settings.sidebarWidth || 282);
      const label = byId("setting-sidebar-width-label");
      if (label) label.textContent = sidebarWidth.value + " px";
      sidebarWidth.addEventListener("input", () => {
        if (label) label.textContent = sidebarWidth.value + " px";
      });
    }
    const https = byId("setting-https");
    if (https) {
      https.checked = Boolean(settings.httpsFirst);
    }
    const clearExit = byId("setting-clear-exit");
    if (clearExit) {
      clearExit.checked = Boolean(settings.clearOnExit);
    }
    const adblock = byId("setting-adblock");
    if (adblock) {
      adblock.checked = settings.adblock !== false;
    }
    const exceptions = byId("setting-adblock-exceptions");
    if (exceptions) {
      exceptions.value = (settings.adblockExceptions || []).join("\n");
    }
    const customRules = byId("setting-adblock-rules");
    if (customRules) {
      customRules.value = (settings.adblockCustomRules || []).join("\n");
    }
    const status = byId("settings-status");
    if (status) {
      status.textContent = "";
    }
    updateAdblockStatus();
  };

  const closeFind = () => {
    const bar = byId("find-bar");
    if (bar) {
      bar.hidden = true;
    }
    send({ type: "ui.find", open: false });
  };

  const queryFind = (forward) => {
    const input = byId("find-input");
    const text = input ? input.value : "";
    send({ type: "find.query", text, forward });
  };

  const openFind = () => {
    const bar = byId("find-bar");
    if (!bar) {
      return;
    }
    bar.hidden = false;
    const input = byId("find-input");
    input.focus();
    input.select();
  };

  const refreshBookmarkTiles = async () => {
    try {
      const data = await request("bookmarks.list");
      current.bookmarks = data.items || [];
      renderShortcuts();
    } catch (error) {
      console.warn(error);
    }
  };

  const showInternalPage = (page) => {
    document.querySelectorAll(".internal-page").forEach((section) => {
      section.hidden = section.dataset.page !== page;
    });
    if (page === "history") {
      loadHistory(byId("history-search")?.value.trim() || "");
    } else if (page === "bookmarks") {
      loadBookmarks();
      refreshBookmarkTiles();
    } else if (page === "settings") {
      populateSettings();
    } else if (page === "downloads") {
      renderDownloads();
    } else if (page === "about") {
      const version = byId("about-version");
      if (version) {
        version.textContent = current.version || "0.0.1";
      }
    }
  };

  const hideInternalPages = () => {
    document.querySelectorAll(".internal-page").forEach((section) => {
      section.hidden = true;
    });
  };

  const addressFullValue = () =>
    current.url || current.displayUrl || "";

  const addressDomainValue = () => {
    if (current.mode === "newtab") {
      return "";
    }
    const raw = current.url || current.displayUrl || "";
    if (!raw || raw.startsWith("rustle://")) {
      return current.displayUrl || raw;
    }
    try {
      return new URL(raw).hostname.replace(/^www\./, "");
    } catch {
      return raw;
    }
  };

  const renderAddress = (forceFull = false) => {
    const address = byId("address");
    if (!address) {
      return;
    }
    const focused = document.activeElement === address;
    const full = forceFull || focused;
    const value = current.mode === "newtab" ? "" : full ? addressFullValue() : addressDomainValue();
    address.value = value;
    address.placeholder = current.mode === "newtab" ? "Поиск или адрес" : "";
    address.classList.toggle("address__input--domain", !full && Boolean(value));
    address.classList.toggle("address__input--full", full && Boolean(value));
  };

  const syncAddressOverlay = () => {
    const box = document.querySelector(".address");
    if (!box) {
      return;
    }
    const rect = box.getBoundingClientRect();
    document.documentElement.style.setProperty("--suggest-left", rect.left + "px");
    document.documentElement.style.setProperty("--suggest-width", rect.width + "px");
  };

  const applyLanguage = (language) => {
    const en = language === "en";
    document.documentElement.lang = en ? "en" : "ru";
    const tagline = document.querySelector(".newtab__tagline");
    if (tagline) tagline.textContent = en ? "Minimal browser. 0.2.0." : "Минималистичный браузер. 0.2.0.";
    const query = byId("newtab-query");
    if (query) query.placeholder = en ? "Search or address" : "Поиск или адрес";
    const add = byId("newtab-add");
    if (add) add.textContent = en ? "+ Add site" : "+ Добавить сайт";
    const edit = byId("newtab-edit");
    if (edit && !tileEdit) edit.textContent = en ? "✎ Edit tiles" : "✎ Редактировать плитки";
  };

  const applyState = (payload) => {
    if (!payload || typeof payload !== "object") {
      return;
    }
    const previousActive = current.activeId;
    current = { ...current, ...payload };
    const activeChanged = payload.activeId !== previousActive;
    if (activeChanged) {
      closeSuggestions();
    }

    if (payload.theme) {
      document.documentElement.dataset.theme = payload.theme;
    }
    const aboutWebview = byId("about-webview2");
    if (aboutWebview && payload.webview2Version) {
      aboutWebview.textContent = payload.webview2Version;
    }
    const aboutUpdate = byId("update-status");
    if (aboutUpdate && payload.updateStatus) {
      aboutUpdate.textContent = payload.updateStatus;
    }
    if (false) {
    }
    if (payload.language) {
      applyLanguage(payload.language);
    }
    if (payload.switcherOpen === true) {
      openSwitcher();
    } else if (payload.switcherOpen === false) {
      hideSwitcher();
    }
    const themeToggle = byId("theme-toggle");
    if (themeToggle) {
      const light = payload.theme === "light";
      const icon = byId("theme-toggle-icon");
      if (icon) icon.textContent = light ? "☀" : "☾";
      themeToggle.title = light ? "Тёмная тема" : "Светлая тема";
    }
    document.documentElement.classList.toggle("private-mode", Boolean(payload.private));
    const about = byId("about");
    if (about) {
      if (payload.private) {
        about.textContent = "PRIVATE";
        about.title = "Приватное окно";
      } else if (current.version) {
        about.textContent = "v" + current.version;
        about.title = "Rustle " + current.version;
      }
    }

    renderTabs(payload.tabs || [], payload.activeId || 0);

    const title = payload.title || "Rustle";
    const address = byId("address");
    if (document.activeElement !== address || activeChanged) {
      renderAddress(false);
    } else {
      renderAddress(true);
    }
    const zoom = Number(payload.zoom);
    if (Number.isFinite(zoom)) {
      document.documentElement.style.setProperty("--page-zoom", String(zoom));
      const zoomLabel = byId("zoom-label");
      if (zoomLabel) {
        zoomLabel.textContent = Math.round(zoom * 100) + "%";
      }
      if (lastZoomPercent !== null && zoom !== lastZoomPercent) {
        showZoomIndicator();
      }
      lastZoomPercent = zoom;
    }

    const secure = Boolean(payload.secure);
    const icon = document.querySelector(".address__icon");
    if (icon) {
      icon.classList.toggle("address__icon--insecure", !secure && String(payload.url || "").startsWith("http://"));
      icon.title = secure ? "HTTPS" : "Соединение не защищено";
    }

    byId("back").disabled = !payload.canGoBack;
    byId("forward").disabled = !payload.canGoForward;

    const loading = Boolean(payload.loading) && (payload.mode || "web") === "web";
    document.documentElement.classList.toggle("loading", loading);

    const mode = payload.mode || "web";
    const internal = mode === "internal";
    const newtab = mode === "newtab";
    const error = payload.error;

    document.documentElement.classList.toggle("newtab-mode", newtab);
    window.__rustleParticles?.[newtab ? "start" : "stop"]?.();
    if (payload.settings) {
      window.__rustleParticles?.configure(payload.settings);
    }
    document.documentElement.classList.toggle("internal-mode", internal);
    document.documentElement.classList.toggle("error-mode", Boolean(error));
    document.documentElement.classList.toggle("vertical-mode", Boolean(payload.vertical));
    document.documentElement.classList.toggle("overlay-mode", mode !== "web");
    const configuredSidebar = Math.max(72, Number(payload.sidebarWidth || 282));
    const sideWidth = payload.vertical ? (payload.sidebarCollapsed ? 56 : configuredSidebar) : 0;
    document.documentElement.style.setProperty("--sidebar-width", sideWidth + "px");
    document.documentElement.style.setProperty("--chrome-offset", payload.vertical ? "44px" : "78px");
    document.documentElement.style.setProperty("--toolbar-nav-left", Math.max(72, sideWidth) + "px");
    window.requestAnimationFrame(syncAddressOverlay);
    if (Array.isArray(payload.sessions)) {
      renderSessions(payload.sessions);
    }
    if (Array.isArray(payload.bookmarks)) {
      current.bookmarks = payload.bookmarks;
      renderShortcuts();
    }
    if (payload.adblockStatus) {
      current.adblockStatus = payload.adblockStatus;
      updateAdblockStatus();
    }

    if (internal) {
      const page = payload.internalPage || "";
      if (page !== currentInternalPage) {
        currentInternalPage = page;
        showInternalPage(page);
      } else if (page === "downloads") {
        renderDownloads();
      }
    } else {
      currentInternalPage = "";
      hideInternalPages();
    }

    const reload = byId("reload");
    reload.dataset.cmd = loading ? "nav.stop" : error ? "nav.retry" : "nav.reload";
    reload.title = loading ? "Остановить" : error ? "Повторить" : "Обновить (F5)";
    reload.setAttribute("aria-label", reload.title);
    reload.disabled = newtab || internal;

    const bookmark = byId("bookmark-current");
    if (bookmark) {
      bookmark.disabled = newtab || internal || Boolean(error);
    }

    if (error) {
      byId("error-code").textContent = "Ошибка · " + (error.code || "unknown");
      byId("error-message").textContent = error.message || "Не удалось открыть страницу.";
      byId("error-url").textContent = payload.displayUrl || payload.url || "";
      byId("error-back").disabled = !payload.canGoBack;
    }

    document.title = title;
  };

  window.addEventListener("resize", syncAddressOverlay);

  let promptState = null;

  const closePrompt = () => {
    const overlay = byId("prompt-overlay");
    if (overlay) overlay.hidden = true;
    promptState = null;
    send({ type: "ui.prompt", open: false });
  };

  const openPrompt = (kind, payload, title, value) => {
    promptState = { kind, payload };
    const overlay = byId("prompt-overlay");
    const titleNode = byId("prompt-title");
    const input = byId("prompt-input");
    if (!overlay || !titleNode || !input) return;
    titleNode.textContent = title;
    input.value = value || "";
    overlay.hidden = false;
    send({ type: "ui.prompt", open: true });
    window.setTimeout(() => { input.focus(); input.select(); }, 30);
  };

  const submitPrompt = () => {
    const input = byId("prompt-input");
    const value = input ? input.value.trim() : "";
    if (!value || !promptState) return;
    const { kind, payload } = promptState;
    closePrompt();
    if (kind === "newgroup") send({ type: "groups.create", name: value, tabId: payload.id });
    else if (kind === "rename") send({ type: "groups.rename", groupId: payload.groupId, name: value });
  };

  const promptInput = byId("prompt-input");
  const promptOk = byId("prompt-ok");
  const promptCancel = byId("prompt-cancel");
  const promptOverlay = byId("prompt-overlay");
  if (promptOk) promptOk.addEventListener("click", submitPrompt);
  if (promptCancel) promptCancel.addEventListener("click", closePrompt);
  if (promptOverlay) promptOverlay.addEventListener("mousedown", (event) => {
    if (event.target === promptOverlay) closePrompt();
  });
  if (promptInput) promptInput.addEventListener("keydown", (event) => {
    if (event.key === "Enter") { event.preventDefault(); submitPrompt(); }
    else if (event.key === "Escape") { event.preventDefault(); closePrompt(); }
  });

  let switcherIndex = 0;
  let switcherItems = [];

  const hideSwitcher = () => {
    const overlay = byId("switcher-overlay");
    if (overlay) overlay.hidden = true;
  };

  const closeSwitcher = () => {
    hideSwitcher();
    send({ type: "ui.switcher", open: false });
  };

  const activateSwitcherItem = (index) => {
    const item = switcherItems[index];
    if (!item) return;
    hideSwitcher();
    send({ type: "ui.switcher", open: false });
    send({ type: "tabs.activate", id: item.id });
  };

  const renderSwitcher = () => {
    const list = byId("switcher-list");
    const input = byId("switcher-input");
    if (!list || !input) return;
    const query = input.value.trim().toLowerCase();
    const fuzzy = (text, needle) => {
      if (!needle) return 1;
      let index = 0;
      let score = 0;
      for (const char of text) {
        if (char === needle[index]) {
          score += index === 0 ? 3 : 1;
          index += 1;
          if (index === needle.length) return score;
        }
      }
      return -1;
    };
    const tabs = (current.tabs || [])
      .map((tab, index) => {
        const title = String(tab.title || "").toLowerCase();
        const url = String(tab.url || "").toLowerCase();
        const score = Math.max(fuzzy(title, query), fuzzy(url, query) - 2);
        return { tab, score, index };
      })
      .filter((entry) => entry.score >= 0)
      .sort((a, b) => b.score - a.score || a.index - b.index)
      .map((entry) => entry.tab);
    switcherItems = tabs;
    switcherIndex = Math.min(switcherIndex, Math.max(0, tabs.length - 1));
    list.replaceChildren();
    tabs.forEach((tab, index) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "switcher__item" + (index === switcherIndex ? " is-active" : "");
      if (tab.faviconData) {
        const img = document.createElement("img");
        img.src = tab.faviconData;
        img.alt = "";
        button.append(img);
      } else {
        const mark = document.createElement("span");
        mark.className = "switcher__mark";
        mark.textContent = String(tab.title || "?").slice(0, 2).toUpperCase();
        button.append(mark);
      }
      const title = document.createElement("span");
      title.className = "switcher__title";
      title.textContent = tab.title || "Без названия";
      const url = document.createElement("span");
      url.className = "switcher__url";
      url.textContent = tab.url || "";
      button.append(title, url);
      button.addEventListener("mousedown", (event) => {
        event.preventDefault();
        activateSwitcherItem(index);
      });
      list.append(button);
    });
  };

  const openSwitcher = () => {
    const overlay = byId("switcher-overlay");
    const input = byId("switcher-input");
    if (!overlay || !input) return;
    switcherIndex = 0;
    overlay.hidden = false;
    input.value = "";
    renderSwitcher();
    window.setTimeout(() => input.focus(), 20);
  };

  const initParticleHero = () => {
    const hero = byId("newtab-hero");
    const canvas = byId("newtab-particles");
    const logo = hero ? hero.querySelector(".newtab__logo") : null;
    const brand = hero ? hero.querySelector(".newtab__brand") : null;
    if (!hero || !canvas || !logo || !brand) return;
    if (window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;

    let particles = [];
    let active = false;
    let frame = 0;
    let enabled = true;
    let intensity = 100;
    let appliedEnabled = true;
    let appliedIntensity = 100;
    let rebuildTimer = 0;
    const mouse = { x: -9999, y: -9999 };

    const build = () => {
      const rect = hero.getBoundingClientRect();
      if (rect.width < 40 || rect.height < 40) return false;
      const width = Math.round(rect.width);
      const height = Math.round(rect.height);
      canvas.width = width;
      canvas.height = height;
      const color = getComputedStyle(brand).color;
      const logoRect = logo.getBoundingClientRect();
      const brandRect = brand.getBoundingClientRect();
      const step = intensity >= 150 ? 1 : intensity <= 60 ? 3 : 2;
      const next = [];

      const textCanvas = document.createElement("canvas");
      textCanvas.width = width;
      textCanvas.height = height;
      const textCtx = textCanvas.getContext("2d", { willReadFrequently: true });
      if (textCtx) {
        const style = getComputedStyle(brand);
        textCtx.font = style.fontWeight + " " + style.fontSize + " " + style.fontFamily;
        textCtx.textBaseline = "top";
        textCtx.fillStyle = "#fff";
        textCtx.fillText(brand.textContent || "Rustle", brandRect.left - rect.left, brandRect.top - rect.top);
        const data = textCtx.getImageData(0, 0, width, height).data;
        for (let y = 0; y < height; y += step) {
          for (let x = 0; x < width; x += step) {
            if (data[(y * width + x) * 4 + 3] > 100) {
              next.push({ x, y, ox: x, oy: y, vx: 0, vy: 0, size: 1.15, color });
            }
          }
        }
      }

      const logoCanvas = document.createElement("canvas");
      logoCanvas.width = width;
      logoCanvas.height = height;
      const logoCtx = logoCanvas.getContext("2d", { willReadFrequently: true });
      if (logoCtx) {
        logoCtx.drawImage(logo, logoRect.left - rect.left, logoRect.top - rect.top, logoRect.width, logoRect.height);
        const data = logoCtx.getImageData(0, 0, width, height).data;
        for (let y = 0; y < height; y += step) {
          for (let x = 0; x < width; x += step) {
            const i = (y * width + x) * 4;
            const luma = (data[i] + data[i + 1] + data[i + 2]) / 3;
            if (data[i + 3] > 100 && luma < 210) {
              next.push({ x, y, ox: x, oy: y, vx: 0, vy: 0, size: 1.35, color });
            }
          }
        }
      }

      if (next.length > 5200) {
        const stride = Math.ceil(next.length / 5200);
        particles = next.filter((_, index) => index % stride === 0);
      } else {
        particles = next;
      }
      return particles.length > 0;
    };

    const draw = () => {
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      for (const particle of particles) {
        particle.vx += (particle.ox - particle.x) * 0.075;
        particle.vy += (particle.oy - particle.y) * 0.075;
        particle.vx *= 0.86;
        particle.vy *= 0.86;
        particle.x += particle.vx;
        particle.y += particle.vy;
        const dx = particle.x - mouse.x;
        const dy = particle.y - mouse.y;
        const distance = Math.hypot(dx, dy);
        if (distance < 54 && distance > 0.1) {
          const force = ((54 - distance) / 54) * 3.4;
          particle.vx += (dx / distance) * force;
          particle.vy += (dy / distance) * force;
        }
        ctx.globalAlpha = 0.92;
        ctx.fillStyle = particle.color;
        ctx.fillRect(particle.x, particle.y, particle.size, particle.size);
      }
      ctx.globalAlpha = 1;
    };

    const loop = () => {
      draw();
      frame = window.requestAnimationFrame(loop);
    };

    const start = () => {
      if (!enabled || active) return;
      if (!build()) {
        window.setTimeout(start, 250);
        return;
      }
      hero.classList.add("is-particles");
      canvas.style.opacity = "1";
      active = true;
      window.cancelAnimationFrame(frame);
      loop();
    };

    const stop = () => {
      active = false;
      window.cancelAnimationFrame(frame);
      hero.classList.remove("is-particles");
      canvas.style.opacity = "0";
      canvas.getContext("2d")?.clearRect(0, 0, canvas.width, canvas.height);
    };

    const configure = (settings) => {
      if (!settings) return;
      if (typeof settings.particlesEnabled === "boolean") enabled = settings.particlesEnabled;
      if (settings.particleIntensity) intensity = Number(settings.particleIntensity);
      if (enabled === appliedEnabled && intensity === appliedIntensity) return;
      appliedEnabled = enabled;
      appliedIntensity = intensity;
      if (!enabled) {
        stop();
        return;
      }
      if (active) {
        stop();
        window.setTimeout(start, 20);
      }
    };

    hero.addEventListener("pointermove", (event) => {
      if (!active) start();
      const rect = hero.getBoundingClientRect();
      mouse.x = event.clientX - rect.left;
      mouse.y = event.clientY - rect.top;
    });
    hero.addEventListener("pointerleave", () => {
      mouse.x = -9999;
      mouse.y = -9999;
    });
    logo.addEventListener("load", () => {
      if (enabled) {
        stop();
        window.setTimeout(start, 20);
      }
    });
    window.addEventListener("resize", () => {
      window.clearTimeout(rebuildTimer);
      rebuildTimer = window.setTimeout(() => {
        if (enabled) {
          stop();
          start();
        }
      }, 180);
    });
    window.__rustleParticles = { start, stop, configure };
    window.setTimeout(start, 200);
  };

  const handleMenuAction = (action) => {
    if (!action || typeof action !== "string") {
      return;
    }
    const parts = action.split(/[.:]/);
    const kind = parts[0];
    const name = parts[1];
    const id = Number(parts[2] ?? parts[1]);

    if (kind === "app") {
      const pages = {
        history: "rustle://history",
        bookmarks: "rustle://bookmarks",
        downloads: "rustle://downloads",
        settings: "rustle://settings",
      };
      if (pages[name]) {
        send({ type: "nav.go", url: pages[name] });
      } else if (name === "newtab") {
        send({ type: "tabs.new" });
      } else if (name === "reopen") {
        send({ type: "tabs.reopen" });
      } else if (name === "screenshot") {
        send({ type: "page.screenshot" });
      } else if (name === "pdf") {
        send({ type: "page.pdf" });
      } else if (name === "whitelist") {
        send({ type: "adblock.whitelist" });
      } else if (name === "sessions") {
        send({ type: "nav.go", url: "rustle://settings" });
        send({ type: "sessions.refresh" });
      } else if (name === "session.save") {
        send({ type: "session.saveAs" });
      } else if (name === "session.open") {
        send({ type: "session.openAs" });
      } else if (name === "autogroup") {
        send({ type: "groups.autogroup" });
      } else if (name === "vertical") {
        send({ type: "ui.vertical", enabled: !current.vertical });
      } else if (name === "bookmark") {
        if (!current.url || !/^https?:\/\//.test(current.url)) {
          return;
        }
        request("bookmarks.toggle", {
          url: current.url,
          title: current.title || current.url,
        })
          .then(() => refreshBookmarkTiles())
          .catch((error) => console.warn(error));
      }
      return;
    }

    if (kind === "tab") {
      if (name === "pin") {
        send({ type: "tabs.pin", id, pinned: true });
      } else if (name === "unpin") {
        send({ type: "tabs.pin", id, pinned: false });
      } else if (name === "mute") {
          send({ type: "tabs.mute", id });
        } else if (name === "close") {
        send({ type: "tabs.close", id });
      } else if (name === "ungroup") {
        send({ type: "tabs.moveGroup", id, groupId: null });
      } else if (name === "group") {
        send({ type: "tabs.moveGroup", id, groupId: Number(parts[3]) });
      } else if (name === "newgroup") {
        openPrompt("newgroup", { id }, "Название группы", "Новая группа");
      }
      return;
    }

    if (kind === "group") {
      const groupId = Number(parts[2]);
      const group = (current.groups || []).find((entry) => entry.id === groupId);
      if (name === "toggle") {
        send({ type: "groups.toggle", groupId });
      } else if (name === "newtab") {
        send({ type: "groups.newTab", groupId });
      } else if (name === "rename") {
        openPrompt("rename", { groupId }, "Название группы", group ? group.name : "Группа");
      } else if (name === "remove") {
        if (window.confirm("Удалить группу «" + (group ? group.name : "") + "» и закрыть её вкладки?")) {
          send({ type: "groups.remove", groupId });
        }
      }
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
          about.textContent = "v" + message.version;
          about.title = "Rustle " + message.version + " (" + message.platform + ")";
        }
        current.version = message.version;
      } else if (message.type === "state") {
        applyState(message.payload);
      } else if (message.type === "find.open") {
        openFind();
      } else if (message.type === "menu.action") {
        handleMenuAction(message.action);
      } else if (message.type === "storage.response") {
        const waiter = pending.get(message.id);
        if (!waiter) {
          return;
        }
        pending.delete(message.id);
        const payload = message.payload || {};
        if (payload.ok === false) {
          waiter.reject(new Error(payload.error || "storage error"));
        } else {
          waiter.resolve(payload);
        }
      }
    },
  };

  const init = () => {
    const address = byId("address");

    address.addEventListener("focus", () => { renderAddress(true); address.select(); });
    address.addEventListener("click", () => { renderAddress(true); address.select(); });
    address.addEventListener("blur", () => renderAddress(false));
    address.addEventListener("keydown", (event) => {
      const panel = byId("suggestions");
      const open = panel && !panel.hidden;
      if (open && event.key === "ArrowDown") {
        event.preventDefault();
        selectSuggestion(1);
        return;
      }
      if (open && event.key === "ArrowUp") {
        event.preventDefault();
        selectSuggestion(-1);
        return;
      }
      if (event.key === "Enter") {
        event.preventDefault();
        if (open && suggestionIndex >= 0 && suggestionItems[suggestionIndex]) {
          chooseSuggestion(suggestionItems[suggestionIndex]);
          return;
        }
        closeSuggestions();
        const url = address.value;
        address.blur();
        send({ type: "nav.go", url });
      } else if (event.key === "Escape") {
        closeSuggestions();
        renderAddress(true);
        address.blur();
      }
    });

    let suggestTimer = 0;
    address.addEventListener("input", () => {
      address.classList.remove("address__input--domain");
      address.classList.add("address__input--full");
      clearTimeout(suggestTimer);
      const query = address.value.trim();
      if (query.length < 2 || query.startsWith("rustle://")) {
        closeSuggestions();
        return;
      }
      suggestTimer = setTimeout(async () => {
        try {
          const data = await request("suggest.query", { query, limit: 8 });
          if (address.value.trim() !== query) {
            return;
          }
          renderSuggestions(data.items || []);
        } catch (error) {
          closeSuggestions();
          console.warn(error);
        }
      }, 140);
    });
    address.addEventListener("blur", () => {
      setTimeout(closeSuggestions, 120);
    });

    document.querySelectorAll("[data-cmd]").forEach((button) => {
      button.addEventListener("click", () => {
        send({ type: button.dataset.cmd });
      });
    });

    document.addEventListener("click", (event) => {
      const nav = event.target.closest("[data-nav]");
      if (nav) {
        event.preventDefault();
        send({ type: "nav.go", url: nav.dataset.nav });
      }
    });

    const tabs = byId("tabs");
    tabs.addEventListener("click", (event) => {
      const close = event.target.closest("[data-close]");
      if (close) {
        event.stopPropagation();
        send({ type: "tabs.close", id: Number(close.dataset.close) });
        return;
      }
      const groupNew = event.target.closest("[data-group-new]");
      if (groupNew) {
        event.stopPropagation();
        send({ type: "groups.newTab", groupId: Number(groupNew.dataset.groupNew) });
        return;
      }
      const groupToggle = event.target.closest("[data-group-toggle]");
      if (groupToggle) {
        event.stopPropagation();
        send({ type: "groups.toggle", groupId: Number(groupToggle.dataset.groupToggle) });
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
      if (event.key !== "Enter" && event.key !== " ") {
        return;
      }
      const groupToggle = event.target.closest("[data-group-toggle]");
      if (groupToggle) {
        event.preventDefault();
        send({ type: "groups.toggle", groupId: Number(groupToggle.dataset.groupToggle) });
        return;
      }
      const tab = event.target.closest(".tab");
      if (tab) {
        event.preventDefault();
        send({ type: "tabs.activate", id: Number(tab.dataset.id) });
      }
    });
    document.addEventListener("wheel", (event) => {
      if (!event.ctrlKey) {
        return;
      }
      event.preventDefault();
      send({ type: "zoom.wheel", delta: event.deltaY });
    }, { passive: false });

    byId("new-tab").addEventListener("click", () => send({ type: "tabs.new" }));

    const go = byId("go");
    if (go) {
      go.addEventListener("click", () => {
        const url = address.value;
        address.blur();
        send({ type: "nav.go", url });
      });
    }

    const bookmark = byId("bookmark-current");
    if (bookmark) {
      bookmark.addEventListener("click", async () => {
        if (!current.url || current.url.startsWith("rustle://")) {
          return;
        }
        try {
          const result = await request("bookmarks.toggle", {
            url: current.url,
            title: current.title || current.url,
          });
          bookmark.classList.toggle("is-active", Boolean(result.bookmarked));
          bookmark.title = result.bookmarked ? "Убрать из закладок" : "Добавить в закладки";
          refreshBookmarkTiles();
        } catch (error) {
          console.warn(error);
        }
      });
    }

    const newtabForm = byId("newtab-search");
    if (newtabForm) {
      newtabForm.addEventListener("click", (event) => {
        if (event.target.closest("button")) {
          return;
        }
        byId("newtab-query").focus();
      });
      newtabForm.addEventListener("submit", (event) => {
        event.preventDefault();
        const query = byId("newtab-query").value.trim();
        if (query) {
          send({ type: "nav.go", url: query });
        }
      });
    }

    renderShortcuts();
    const addShortcut = byId("newtab-add");
    if (addShortcut) {
      addShortcut.addEventListener("click", () => {
        const raw = window.prompt("Адрес сайта", "https://");
        if (!raw) {
          return;
        }
        let url = raw.trim();
        if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(url)) {
          url = "https://" + url;
        }
        const title = window.prompt("Название плитки", url) || url;
        const items = readShortcuts();
        items.push({
          url,
          title,
          mark: title.slice(0, 2).toUpperCase(),
        });
        saveShortcuts(items);
        renderShortcuts();
      });
    }

    const importBookmarks = byId("bookmarks-import");
    if (importBookmarks) {
      importBookmarks.addEventListener("click", async () => {
        await request("bookmarks.import");
        loadBookmarks();
        refreshBookmarkTiles();
      });
    }

    const exportBookmarks = byId("bookmarks-export");
    if (exportBookmarks) {
      exportBookmarks.addEventListener("click", async () => {
        const data = await request("bookmarks.list");
        await request("bookmarks.export", { entries: data.items || [] });
      });
    }

    const historySearch = byId("history-search");
    if (historySearch) {
      let timer = 0;
      historySearch.addEventListener("input", () => {
        clearTimeout(timer);
        timer = setTimeout(() => loadHistory(historySearch.value.trim()), 180);
      });
    }

    const historyClear = byId("history-clear");
    if (historyClear) {
      historyClear.addEventListener("click", async () => {
        if (!window.confirm("Очистить всю историю?")) {
          return;
        }
        await request("history.clear");
        loadHistory(byId("history-search").value.trim());
      });
    }

    const engineSelect = byId("setting-engine");
    if (engineSelect) {
      engineSelect.addEventListener("change", () => {
        send({ type: "settings.save", searchEngine: engineSelect.value });
      });
    }

    const settingsForm = byId("settings-form");
    if (settingsForm) {
      settingsForm.addEventListener("submit", (event) => {
        event.preventDefault();
        send({
          type: "settings.save",
          searchEngine: byId("setting-engine").value,
          theme: byId("setting-theme").value,
          startup: byId("setting-startup").value,
          httpsFirst: byId("setting-https").checked,
          clearOnExit: byId("setting-clear-exit").checked,
          adblock: byId("setting-adblock").checked,
            sidebarWidth: Number(byId("setting-sidebar-width")?.value || 282),
            adblockCustomRules: (byId("setting-adblock-rules")?.value || "")
              .split(/\r?\n/)
              .map((entry) => entry.trim())
              .filter(Boolean),
            adblockExceptions: (byId("setting-adblock-exceptions")?.value || "")
              .split(/[,\n]+/)
              .map((entry) => entry.trim())
              .filter(Boolean),
        });
        byId("settings-status").textContent = "Сохранено";
      });
    }

    const adblockUpdate = byId("adblock-update");
    if (adblockUpdate) {
      adblockUpdate.addEventListener("click", () => {
        send({ type: "adblock.update" });
        const status = byId("adblock-status");
        if (status) {
          status.textContent = "Обновление фильтров…";
        }
        adblockUpdate.disabled = true;
        adblockUpdate.textContent = "Обновление…";
      });
    }

    const findInput = byId("find-input");
    if (findInput) {
      findInput.addEventListener("input", () => queryFind(true));
      findInput.addEventListener("keydown", (event) => {
        if (event.key === "Escape") {
          closeFind();
        } else if (event.key === "Enter") {
          event.preventDefault();
          queryFind(!event.shiftKey);
        }
      });
    }
    const findNext = byId("find-next");
    if (findNext) {
      findNext.addEventListener("click", () => queryFind(true));
    }
    const findPrev = byId("find-prev");
    if (findPrev) {
      findPrev.addEventListener("click", () => queryFind(false));
    }
    const findClose = byId("find-close");
    if (findClose) {
      findClose.addEventListener("click", closeFind);
    }

    const clearData = byId("clear-data");
    if (clearData) {
      clearData.addEventListener("click", () => {
        if (window.confirm("Очистить cookies и кэш WebView2?")) {
          send({ type: "data.clear" });
          byId("settings-status").textContent = "Очищено";
        }
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

    const menuToggle = byId("menu-toggle");
    if (menuToggle) {
      menuToggle.addEventListener("click", (event) => {
        event.stopPropagation();
        const rect = menuToggle.getBoundingClientRect();
        send({
          type: "ui.menu",
          kind: "app",
          origin: "chrome",
          x: rect.left,
          y: rect.bottom,
        });
      });
    }
    document.addEventListener("contextmenu", (event) => {
      if (event.target.closest("input, textarea, select")) {
        return;
      }
      const tab = event.target.closest(".tab");
      if (tab) {
        event.preventDefault();
        send({
          type: "ui.menu",
          kind: "tab",
          id: Number(tab.dataset.id),
          origin: "chrome",
          x: event.clientX,
          y: event.clientY,
        });
        return;
      }
      const group = event.target.closest(".tab-group");
      if (group) {
        event.preventDefault();
        send({
          type: "ui.menu",
          kind: "group",
          groupId: Number(group.dataset.groupId),
          origin: "chrome",
          x: event.clientX,
          y: event.clientY,
        });
        return;
      }
      if (event.target.closest(".toolbar, .tabs")) {
        event.preventDefault();
        send({
          type: "ui.menu",
          kind: "app",
          origin: "chrome",
          x: event.clientX,
          y: event.clientY,
        });
      }
    });
    const toolbarHome = byId("toolbar-home");
    if (toolbarHome) {
      toolbarHome.addEventListener("click", () => send({ type: "nav.go", url: "rustle://newtab" }));
    }
    const tilesEdit = byId("newtab-edit");
    if (tilesEdit) {
      tilesEdit.addEventListener("click", () => {
        tileEdit = !tileEdit;
        tilesEdit.classList.toggle("is-active", tileEdit);
        tilesEdit.textContent = tileEdit ? "✓ Готово" : "✎ Редактировать плитки";
        renderShortcuts();
      });
    }

    const themeButton = byId("theme-toggle");
    if (themeButton) {
      themeButton.addEventListener("click", () => send({ type: "ui.theme" }));
    }

    const windowControls = byId("window-controls");
    let windowDragOrigin = null;
    let windowDragStarted = false;
    let windowDragTimer = 0;
    let suppressWindowClick = false;
    if (windowControls) {
      windowControls.addEventListener("pointerdown", (event) => {
        if (event.button !== 0) {
          return;
        }
        windowDragOrigin = { x: event.clientX, y: event.clientY };
        windowDragStarted = false;
        window.clearTimeout(windowDragTimer);
        windowDragTimer = window.setTimeout(() => {
          if (!windowDragOrigin || windowDragStarted) {
            return;
          }
          windowDragStarted = true;
          suppressWindowClick = true;
          send({ type: "window.drag" });
        }, 190);
      });
      windowControls.addEventListener(
        "click",
        (event) => {
          if (suppressWindowClick) {
            event.preventDefault();
            event.stopPropagation();
            suppressWindowClick = false;
          }
        },
        true,
      );
    }
    document.addEventListener("pointermove", (event) => {
      if (!windowDragOrigin || windowDragStarted) {
        return;
      }
      const dx = event.clientX - windowDragOrigin.x;
      const dy = event.clientY - windowDragOrigin.y;
      if (Math.hypot(dx, dy) < 5) {
        return;
      }
      windowDragStarted = true;
      suppressWindowClick = true;
      window.clearTimeout(windowDragTimer);
      send({ type: "window.drag" });
    });
    document.addEventListener("pointerup", () => {
      window.clearTimeout(windowDragTimer);
      windowDragOrigin = null;
      windowDragStarted = false;
    });

    const minimize = byId("win-min");
    if (minimize) {
      minimize.addEventListener("click", () => send({ type: "window.min" }));
    }
    const maximize = byId("win-max");
    if (maximize) {
      maximize.addEventListener("click", () => send({ type: "window.max" }));
    }
    const closeWindow = byId("win-close");
    if (closeWindow) {
      closeWindow.addEventListener("click", () => send({ type: "window.close" }));
    }
    const toolbar = document.querySelector(".toolbar");
    if (toolbar) {
      toolbar.addEventListener("mousedown", (event) => {
        if (
          event.button === 0 &&
          !event.target.closest("button, input, .address, [role='button']")
        ) {
          send({ type: "window.drag" });
        }
      });
      toolbar.addEventListener("dblclick", (event) => {
        if (!event.target.closest("button, input, .address, [role='button']")) {
          send({ type: "window.max" });
        }
      });
    }

    const tabStrip = document.querySelector(".tabs");
    if (tabStrip) {
      tabStrip.addEventListener("mousedown", (event) => {
        if (
          event.button === 0 &&
          (event.target === tabStrip ||
            event.target.classList.contains("tabs__items") ||
            event.target.classList.contains("tabs__spacer"))
        ) {
          send({ type: "window.drag" });
        }
      });
      tabStrip.addEventListener("dblclick", (event) => {
        if (event.target === tabStrip || event.target.classList.contains("tabs__spacer")) {
          send({ type: "window.max" });
        }
      });
    }

    const zoomLabel = byId("zoom-label");
    if (zoomLabel) {
      const reset = () => {
        showZoomIndicator();
        send({ type: "zoom.reset" });
      };
      zoomLabel.addEventListener("click", reset);
      zoomLabel.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          reset();
        }
      });
    }

    const translate = byId("translate-page");
    if (translate) {
      translate.addEventListener("click", () => {
        send({ type: "tabs.newUrl", url: "https://translate.google.com/" });
      });
    }

    const switcherInput = byId("switcher-input");
    const switcherOverlay = byId("switcher-overlay");
    if (switcherInput) {
      switcherInput.addEventListener("input", () => {
        switcherIndex = 0;
        renderSwitcher();
      });
      switcherInput.addEventListener("keydown", (event) => {
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          event.preventDefault();
          if (!switcherItems.length) return;
          switcherIndex = (switcherIndex + (event.key === "ArrowDown" ? 1 : -1) + switcherItems.length) % switcherItems.length;
          renderSwitcher();
          const active = document.querySelector(".switcher__item.is-active");
          if (active) active.scrollIntoView({ block: "nearest" });
        } else if (event.key === "Enter") {
          event.preventDefault();
          activateSwitcherItem(switcherIndex);
        } else if (event.key === "Escape") {
          event.preventDefault();
          closeSwitcher();
        }
      });
    }
    if (switcherOverlay) {
      switcherOverlay.addEventListener("mousedown", (event) => {
        if (event.target === switcherOverlay) closeSwitcher();
      });
    }

    initParticleHero();

    send({ type: "chrome.ready" });
  };

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init, { once: true });
  } else {
    init();
  }
})();