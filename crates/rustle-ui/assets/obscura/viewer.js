(function () {
  "use strict";
  var tabs = document.getElementById("ob-tabs");
  var screen = document.getElementById("ob-screen");
  var status = document.getElementById("ob-status");
  var address = document.getElementById("ob-address");
  var profile = document.getElementById("ob-profile");
  var editing = false;
  var state = {};

  function send(message) {
    if (window.ipc) window.ipc.postMessage(JSON.stringify(message));
  }

  function renderTabs() {
    tabs.replaceChildren();
    (state.tabs || []).forEach(function (tab) {
      var button = document.createElement("button");
      button.type = "button";
      button.className = "ob-tab" + (tab.active ? " active" : "");
      button.title = tab.url || tab.title || "";
      var label = document.createElement("span");
      label.textContent = tab.title || tab.url || "Вкладка";
      button.appendChild(label);
      button.addEventListener("click", function () {
        send({ type: "obscura.activate", id: tab.id });
      });
      var close = document.createElement("button");
      close.type = "button";
      close.className = "ob-close";
      close.textContent = "×";
      close.addEventListener("click", function (event) {
        event.stopPropagation();
        send({ type: "obscura.close", id: tab.id });
      });
      button.appendChild(close);
      tabs.appendChild(button);
    });
  }

  function renderProfiles() {
    var selected = String(state.profile || "");
    profile.replaceChildren();
    (state.profiles || []).forEach(function (item) {
      var option = document.createElement("option");
      option.value = String(item.id);
      option.textContent = item.name || ("Профиль " + item.id);
      if (String(item.id) === selected) option.selected = true;
      profile.appendChild(option);
    });
  }

  function update(next) {
    state = next || {};
    renderTabs();
    renderProfiles();
    if (state.status) status.textContent = state.status;
    if (state.error) {
      status.textContent = state.error;
      status.style.display = "block";
    }
    if (state.screenshot) {
      screen.src = state.screenshot;
      status.style.display = "none";
    } else {
      screen.removeAttribute("src");
      status.style.display = "block";
    }
    if (!editing) {
      var active = (state.tabs || []).find(function (tab) { return tab.active; });
      address.value = active ? (active.url || "") : "";
    }
  }

  window.rustleObscura = { update: update };

  document.getElementById("ob-new").addEventListener("click", function () {
    send({ type: "obscura.newTab", url: "about:blank" });
  });
  document.getElementById("ob-reload").addEventListener("click", function () {
    send({ type: "obscura.reload" });
  });
  document.getElementById("ob-go").addEventListener("click", function () {
    var url = address.value.trim();
    if (url) send({ type: "obscura.navigate", url: url });
  });
  profile.addEventListener("change", function () {
    send({ type: "obscura.profile", id: Number(profile.value) });
  });
  address.addEventListener("focus", function () { editing = true; });
  address.addEventListener("blur", function () { editing = false; });
  address.addEventListener("keydown", function (event) {
    if (event.key !== "Enter") return;
    event.preventDefault();
    var url = address.value.trim();
    if (!url) return;
    send({ type: "obscura.navigate", url: url });
  });
  screen.addEventListener("click", function (event) {
    var rect = screen.getBoundingClientRect();
    if (!rect.width || !rect.height) return;
    var x = (event.clientX - rect.left) / rect.width * (screen.naturalWidth || rect.width);
    var y = (event.clientY - rect.top) / rect.height * (screen.naturalHeight || rect.height);
    send({ type: "obscura.click", x: x, y: y });
  });
  document.addEventListener("keydown", function (event) {
    if (editing) return;
    if (event.key === "Enter" || event.key === "Backspace" || event.key.length === 1) {
      send({ type: "obscura.key", key: event.key });
    }
  });
  send({ type: "obscura.start" });
})();
