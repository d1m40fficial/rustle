(() => {
  const send = (message) => {
    if (!window.ipc) {
      console.warn("IPC is not available");
      return;
    }
    window.ipc.postMessage(JSON.stringify(message));
  };

  window.rustle = {
    dispatch(message) {
      if (!message || typeof message.type !== "string") {
        return;
      }
      if (message.type === "app.info") {
        const about = document.getElementById("about");
        if (about) {
          about.textContent = "M0 · " + message.version;
          about.title = "Rustle " + message.version + " (" + message.platform + ")";
        }
      }
    },
  };

  const init = () => {
    const address = document.getElementById("address");

    address.addEventListener("focus", () => address.select());
    address.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        send({ type: "nav.go", url: address.value });
      } else if (event.key === "Escape") {
        address.blur();
      }
    });

    document.querySelectorAll("[data-cmd]").forEach((button) => {
      button.addEventListener("click", () => {
        send({ type: button.dataset.cmd });
      });
    });

    const go = document.getElementById("go");
    if (go) {
      go.addEventListener("click", () => {
        send({ type: "nav.go", url: address.value });
      });
    }

    const toggle = document.getElementById("overlay-toggle");
    if (toggle) {
      toggle.addEventListener("click", () => {
        send({ type: "ui.toggleOverlay" });
      });
    }

    const close = document.getElementById("overlay-close");
    if (close) {
      close.addEventListener("click", () => {
        send({ type: "ui.toggleOverlay" });
      });
    }

    send({ type: "chrome.ready" });
  };

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init, { once: true });
  } else {
    init();
  }
})();
