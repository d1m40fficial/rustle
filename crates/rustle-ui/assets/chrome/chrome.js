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
      if (tab.favicon) {
        favicon.classList.add("tab__favicon--image");
        const image = document.createElement("img");
        image.src = "rustle://localhost/favicon/" + tab.favicon;
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
                loadBookmarkBar();
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
    { url: "https://github.com/d1m40fficial/rustle", title: "GitHub", mark: "GH" },
    { url: "https://www.youtube.com", title: "YouTube", mark: "YT" },
    { url: "https://ru.wikipedia.org", title: "Википедия", mark: "W" },
  ];

  const shortcutsKey = "rustle.shortcuts.v1";

  const readShortcuts = () => {
    try {
      const raw = localStorage.getItem(shortcutsKey);
      const parsed = raw ? JSON.parse(raw) : null;
      if (Array.isArray(parsed) && parsed.length) {
        return parsed;
      }
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

  const renderShortcuts = () => {
    const container = byId("newtab-shortcuts");
    if (!container) {
      return;
    }
    const items = readShortcuts();
    container.replaceChildren();
    items.forEach((item, index) => {
      const tile = document.createElement("div");
      tile.className = "newtab__shortcut";
      tile.tabIndex = 0;
      tile.setAttribute("role", "button");
      const mark = document.createElement("span");
      mark.className = "newtab__mark";
      mark.textContent = item.mark || (item.title || item.url).slice(0, 2).toUpperCase();
      const label = document.createElement("span");
      label.textContent = item.title || item.url;
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "shortcut__remove";
      remove.title = "Удалить плитку";
      remove.setAttribute("aria-label", "Удалить плитку");
      remove.textContent = "×";
      remove.addEventListener("click", (event) => {
        event.stopPropagation();
        const next = readShortcuts().filter((_, i) => i !== index);
        saveShortcuts(next);
        renderShortcuts();
      });
      tile.append(mark, label, remove);
      tile.addEventListener("click", () => send({ type: "nav.go", url: item.url }));
      tile.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          send({ type: "nav.go", url: item.url });
        }
      });
      container.append(tile);
    });
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
    const status = byId("settings-status");
    if (status) {
      status.textContent = "";
    }
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

  const renderBookmarkBar = (items, visible = true) => {
    const bar = byId("bookmark-bar");
    if (!bar) {
      return;
    }
    bar.replaceChildren();
    if (!visible || !items.length) {
      bar.hidden = true;
      document.documentElement.style.setProperty("--chrome-offset", "78px");
      return;
    }
    bar.hidden = false;
    document.documentElement.style.setProperty("--chrome-offset", "108px");
    items.slice(0, 16).forEach((item) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "bookmark-bar__item";
      button.textContent = item.title || item.url;
      button.title = item.url;
      button.addEventListener("click", () => send({ type: "nav.go", url: item.url }));
      bar.append(button);
    });
  };

  const loadBookmarkBar = async () => {
    try {
      const data = await request("bookmarks.list");
      renderBookmarkBar(data.items || [], current.bookmarkBarVisible !== false);
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
      loadBookmarkBar();
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
    const zoomLabel = byId("zoom-label");
    if (zoomLabel && Number.isFinite(zoom)) {
      zoomLabel.textContent = Math.round(zoom * 100) + "%";
    }

    const secure = Boolean(payload.secure);
    const icon = document.querySelector(".address__icon");
    if (icon) {
      icon.classList.toggle("address__icon--insecure", !secure && String(payload.url || "").startsWith("http://"));
      icon.title = secure ? "HTTPS" : "Соединение не защищено";
    }

    byId("back").disabled = !payload.canGoBack;
    byId("forward").disabled = !payload.canGoForward;

    const loading = Boolean(payload.loading);
    document.documentElement.classList.toggle("loading", loading);

    const mode = payload.mode || "web";
    const internal = mode === "internal";
    const newtab = mode === "newtab";
    const error = payload.error;

    document.documentElement.classList.toggle("newtab-mode", newtab);
    document.documentElement.classList.toggle("internal-mode", internal);
    document.documentElement.classList.toggle("error-mode", Boolean(error));
    document.documentElement.classList.toggle("vertical-mode", Boolean(payload.vertical));
    document.documentElement.classList.toggle("overlay-mode", mode !== "web");

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
          loadBookmarkBar();
        } catch (error) {
          console.warn(error);
        }
      });
    }

    const newtabForm = byId("newtab-search");
    if (newtabForm) {
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
        loadBookmarkBar();
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
        });
        byId("settings-status").textContent = "Сохранено";
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
    const appMenu = byId("app-menu");
    const closeMenu = () => {
      if (appMenu && !appMenu.hidden) {
        appMenu.hidden = true;
        menuToggle?.setAttribute("aria-expanded", "false");
        send({ type: "ui.menu", open: false });
      }
    };
    if (menuToggle && appMenu) {
      menuToggle.addEventListener("click", (event) => {
        event.stopPropagation();
        const open = appMenu.hidden;
        appMenu.hidden = !open;
        menuToggle.setAttribute("aria-expanded", String(open));
        send({ type: "ui.menu", open });
      });
      appMenu.addEventListener("click", (event) => {
        if (event.target.closest("[data-nav]")) {
          closeMenu();
        }
      });
      document.addEventListener("click", (event) => {
        if (!event.target.closest("#app-menu, #menu-toggle")) {
          closeMenu();
        }
      });
    }
    const menuBookmark = byId("menu-bookmark");
    if (menuBookmark) {
      menuBookmark.addEventListener("click", async () => {
        if (!current.url || !/^https?:\/\//.test(current.url)) {
          closeMenu();
          return;
        }
        try {
          await request("bookmarks.toggle", {
            url: current.url,
            title: current.title || current.url,
          });
          loadBookmarkBar();
        } catch (error) {
          console.warn(error);
        }
        closeMenu();
      });
    }
    const menuBar = byId("menu-bookmarks-bar");
    if (menuBar) {
      menuBar.addEventListener("click", () => {
        send({ type: "ui.bookmarkBarToggle" });
        closeMenu();
      });
    }

    const verticalToggle = byId("vertical-toggle");
    if (verticalToggle) {
      verticalToggle.addEventListener("click", () => {
        send({ type: "ui.vertical", enabled: !current.vertical });
      });
    }

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
      const reset = () => send({ type: "zoom.reset" });
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
        if (!current.url || !/^https?:\/\//.test(current.url)) {
          return;
        }
        send({ type: "tabs.newUrl", url: "https://translate.google.com/" });
      });
    }

    send({ type: "chrome.ready" });
    loadBookmarkBar();
  };

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init, { once: true });
  } else {
    init();
  }
})();